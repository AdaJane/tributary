//! Live MIDI output connections.
//!
//! `midir`'s `MidiOutput`, the sibling of [`crate::midi_in`]'s
//! `MidiInput`, over the same ALSA sequencer and behind the same feature
//! gate. No new crate and no new system package: `midir` was already here
//! for the input half.
//!
//! Two implementations behind one shape, for the same reason the input
//! side has them — a `--no-default-features` build still has to compile
//! and still has to say honestly that it has no ports.

use trib_core::MidiRoute;

use crate::midi_echo::Outgoing;

/// Ports a route names, in first-mention order.
///
/// Derived from the DOCUMENT, never from enumeration order: a module that
/// re-enumerates in a different position must not change what it plays.
pub fn index_routes(routes: &[MidiRoute]) -> Vec<String> {
    let mut ports: Vec<String> = Vec::new();
    for route in routes {
        if !ports.iter().any(|p| p == &route.port) {
            ports.push(route.port.clone());
        }
    }
    ports
}

/// One MIDI output port, joined with what the daemon knows about it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct MidiOutPortReport {
    /// The port's NAME — stable across a replug, unlike its client number.
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// A route names it but the system does not offer it.
    pub absent: bool,
    /// Routes feeding it. More than one is a merge, which is legal here
    /// and worth drawing.
    pub routes: u16,
    /// Messages sent since it opened.
    ///
    /// The only thing that distinguishes "wired and nobody is playing"
    /// from "wired wrong" — an output port has no meter, so without a
    /// counter a dead cable and a quiet keyboard look identical.
    pub sent: u64,
    /// Writes the port refused. An interface that went away mid-song shows
    /// up here before it shows up as absent.
    pub errors: u64,
}

/// Build the report from what the document wants and what the system
/// offers. Pure, so the two implementations cannot disagree about it.
pub fn report_out_ports(
    available: &[String],
    routes: &[MidiRoute],
    traffic: &[(String, u64, u64)],
) -> Vec<MidiOutPortReport> {
    let count = |name: &str| {
        u16::try_from(routes.iter().filter(|r| r.port == name).count()).unwrap_or(u16::MAX)
    };
    let stats = |name: &str| {
        traffic
            .iter()
            .find(|(port, _, _)| port == name)
            .map_or((0, 0), |(_, sent, errors)| (*sent, *errors))
    };
    let mut out: Vec<MidiOutPortReport> = available
        .iter()
        .map(|name| {
            let (sent, errors) = stats(name);
            MidiOutPortReport {
                id: name.clone(),
                name: name.clone(),
                connected: routes.iter().any(|r| &r.port == name),
                absent: false,
                routes: count(name),
                sent,
                errors,
            }
        })
        .collect();
    // A port a route names but the system does not offer still gets a row:
    // "not connected" has to be visible somewhere, or a route that plays
    // nothing has no explanation.
    for name in index_routes(routes) {
        if !available.iter().any(|a| a == &name) {
            let (sent, errors) = stats(&name);
            out.push(MidiOutPortReport {
                id: name.clone(),
                connected: false,
                absent: true,
                routes: count(&name),
                sent,
                errors,
                name,
            });
        }
    }
    out
}

#[cfg(feature = "hardware")]
mod imp {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use midir::{MidiOutput, MidiOutputConnection};

    use super::*;

    /// Per-port counters, shared with the report.
    #[derive(Debug, Default)]
    struct Traffic {
        sent: AtomicU64,
        errors: AtomicU64,
    }

    pub struct OutPorts {
        /// Behind a lock because this whole object is shared, by `Arc`,
        /// with the midir INPUT callbacks that do the echoing.
        available: Mutex<Vec<String>>,
        /// Held behind one mutex, contended only between the midir INPUT
        /// callbacks and the take feeder — never with the audio thread,
        /// which knows nothing about any of this. `send` on an ALSA
        /// sequencer port is microseconds; if it ever shows in a profile
        /// the refinement is one mutex per port behind the same API.
        open: Mutex<HashMap<String, MidiOutputConnection>>,
        traffic: Mutex<HashMap<String, Arc<Traffic>>>,
    }

    impl Default for OutPorts {
        fn default() -> Self {
            Self::new()
        }
    }

    impl OutPorts {
        pub fn new() -> Self {
            let ports = OutPorts {
                available: Mutex::new(Vec::new()),
                open: Mutex::new(HashMap::new()),
                traffic: Mutex::new(HashMap::new()),
            };
            ports.refresh();
            ports
        }

        pub fn refresh(&self) {
            *self.available.lock().expect("midi out available lock") = enumerate();
        }

        pub fn available(&self) -> Vec<String> {
            self.available
                .lock()
                .expect("midi out available lock")
                .clone()
        }

        /// Open what the routes ask for, close what they no longer do.
        pub fn bind(&self, routes: &[MidiRoute]) {
            let wanted = index_routes(routes);
            let mut open = self.open.lock().expect("midi out lock");
            // Closing a port means dropping the only thing that could ever
            // release its notes, so it gets silenced first.
            let going: Vec<String> = open
                .keys()
                .filter(|name| !wanted.contains(name))
                .cloned()
                .collect();
            for name in going {
                if let Some(mut conn) = open.remove(&name) {
                    silence(&mut conn);
                }
            }
            for name in wanted {
                if open.contains_key(&name) {
                    continue;
                }
                if let Some(conn) = connect(&name) {
                    open.insert(name, conn);
                }
            }
        }

        /// Send one message. Never retries and never blocks: a port that
        /// refuses a write is counted and reported, not waited on.
        pub fn send(&self, out: &Outgoing) {
            let traffic = self.traffic_for(&out.port);
            let mut open = self.open.lock().expect("midi out lock");
            let Some(conn) = open.get_mut(&out.port) else {
                return;
            };
            let message: &[u8] = &[
                (out.status & 0xF0) | (out.channel & 0x0F),
                out.data1 & 0x7F,
                out.data2 & 0x7F,
            ];
            // 0xC0/0xD0 are two-byte messages; sending a third byte makes
            // the stream invalid from that point on.
            let len = if matches!(out.status & 0xF0, 0xC0 | 0xD0) {
                2
            } else {
                3
            };
            match conn.send(&message[..len]) {
                Ok(()) => traffic.sent.fetch_add(1, Ordering::Relaxed),
                Err(_) => traffic.errors.fetch_add(1, Ordering::Relaxed),
            };
        }

        pub fn report(&self, routes: &[MidiRoute]) -> Vec<MidiOutPortReport> {
            let traffic: Vec<(String, u64, u64)> = self
                .traffic
                .lock()
                .expect("midi traffic lock")
                .iter()
                .map(|(name, t)| {
                    (
                        name.clone(),
                        t.sent.load(Ordering::Relaxed),
                        t.errors.load(Ordering::Relaxed),
                    )
                })
                .collect();
            report_out_ports(&self.available(), routes, &traffic)
        }

        /// Silence one port.
        pub fn panic_port(&self, port: &str) {
            let mut open = self.open.lock().expect("midi out lock");
            if let Some(conn) = open.get_mut(port) {
                silence(conn);
            }
        }

        /// Silence every open port.
        pub fn panic(&self) {
            let mut open = self.open.lock().expect("midi out lock");
            for conn in open.values_mut() {
                silence(conn);
            }
        }

        fn traffic_for(&self, port: &str) -> Arc<Traffic> {
            let mut traffic = self.traffic.lock().expect("midi traffic lock");
            traffic.entry(port.to_owned()).or_default().clone()
        }
    }

    impl Drop for OutPorts {
        fn drop(&mut self) {
            self.panic();
        }
    }

    /// Sustain off, THEN all notes off, on every channel.
    ///
    /// Deliberately stronger than the input side's panic, which sends CC
    /// 123 alone. That is enough for a `rustysynth` whose sustain state we
    /// own — but an EXTERNAL synth holding CC 64 keeps sounding straight
    /// through an all-notes-off, and the whole point of a panic button is
    /// that it works on the thing you cannot reach.
    fn silence(conn: &mut MidiOutputConnection) {
        for channel in 0..16u8 {
            let _ = conn.send(&[0xB0 | channel, 64, 0]);
            let _ = conn.send(&[0xB0 | channel, 123, 0]);
        }
    }

    fn connect(name: &str) -> Option<MidiOutputConnection> {
        let output = MidiOutput::new("tributary").ok()?;
        let port = output
            .ports()
            .into_iter()
            .find(|p| output.port_name(p).is_ok_and(|n| n == name))?;
        match output.connect(&port, "tributary-out") {
            Ok(conn) => Some(conn),
            Err(err) => {
                tracing::warn!(port = name, error = %err, "MIDI output port would not open");
                None
            }
        }
    }

    fn enumerate() -> Vec<String> {
        let Ok(output) = MidiOutput::new("tributary-enumerate") else {
            tracing::warn!("no ALSA sequencer: MIDI output is unavailable");
            return Vec::new();
        };
        let mut names: Vec<String> = output
            .ports()
            .iter()
            .filter_map(|p| output.port_name(p).ok())
            // MORE load-bearing here than on the input side: without it a
            // route could point our own output at our own input and build
            // a feedback loop that saturates the event ring in
            // milliseconds.
            .filter(|name| !name.starts_with("tributary"))
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

#[cfg(not(feature = "hardware"))]
mod imp {
    use super::*;

    /// No ALSA, no ports. Routes stay in the document and report absent,
    /// which is the truth rather than an error.
    #[derive(Debug, Default)]
    pub struct OutPorts;

    impl OutPorts {
        pub fn new() -> Self {
            OutPorts
        }
        pub fn refresh(&self) {}
        pub fn available(&self) -> Vec<String> {
            Vec::new()
        }
        pub fn bind(&self, _routes: &[MidiRoute]) {}
        pub fn send(&self, _out: &Outgoing) {}
        pub fn report(&self, routes: &[MidiRoute]) -> Vec<MidiOutPortReport> {
            report_out_ports(&[], routes, &[])
        }
        pub fn panic_port(&self, _port: &str) {}
        pub fn panic(&self) {}
    }
}

pub use imp::OutPorts;

#[cfg(test)]
mod tests {
    use trib_core::MidiSource;

    use super::*;

    fn route(port: &str, name: &str) -> MidiRoute {
        MidiRoute {
            port: port.into(),
            channel: None,
            source: MidiSource::Port { name: name.into() },
        }
    }

    #[test]
    fn a_port_a_route_names_but_nothing_offers_is_reported_absent() {
        // Without this row a route that plays nothing has no explanation
        // anywhere — the same failure the input side's absent rows exist
        // to prevent.
        let report = report_out_ports(&[], &[route("Juno", "nanoKEY2")], &[]);
        assert_eq!(report.len(), 1);
        assert!(report[0].absent);
        assert!(!report[0].connected);
    }

    #[test]
    fn a_merge_reports_how_many_routes_feed_the_port() {
        let routes = vec![route("Juno", "nanoKEY2"), route("Juno", "other")];
        let report = report_out_ports(&["Juno".to_owned()], &routes, &[]);
        assert_eq!(report[0].routes, 2);
        assert!(report[0].connected);
    }

    #[test]
    fn traffic_counters_reach_the_report() {
        let report = report_out_ports(
            &["Juno".to_owned()],
            &[route("Juno", "nanoKEY2")],
            &[("Juno".to_owned(), 42, 3)],
        );
        assert_eq!((report[0].sent, report[0].errors), (42, 3));
    }

    #[test]
    fn an_offered_port_nothing_routes_to_is_listed_but_not_connected() {
        let report = report_out_ports(&["Juno".to_owned()], &[], &[]);
        assert_eq!(report.len(), 1);
        assert!(!report[0].connected);
        assert!(!report[0].absent);
    }

    #[test]
    fn ports_are_indexed_from_the_document_not_from_enumeration() {
        let routes = vec![
            route("Juno", "a"),
            route("Prophet", "b"),
            route("Juno", "c"),
        ];
        assert_eq!(index_routes(&routes), vec!["Juno", "Prophet"]);
    }
}
