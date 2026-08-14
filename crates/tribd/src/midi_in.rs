//! Live MIDI input connections.
//!
//! Two implementations behind one shape. With the `hardware` feature this
//! is `midir` over the ALSA sequencer; without it — the header-less build
//! `cargo build -p tribd --no-default-features` produces — it is a stub
//! that enumerates nothing. Instruments still render in that build, because
//! `rustysynth` is pure Rust; they just have no keyboard, and the tests
//! drive the event ring directly.
//!
//! Ports are identified by NAME, never by ALSA client number: the kernel
//! hands out a fresh number on every replug, so a number would silently
//! rebind an instrument to whatever appeared in that slot.

use rtrb::Producer;
use trib_core::InstrumentState;
use trib_engine::MidiEvent;

use crate::instrument_host::MidiPortReport;

/// A note the test button plays: middle C, medium velocity, brief.
const TEST_KEY: u8 = 60;
const TEST_VELOCITY: u8 = 96;

/// Assign a stable index to each port an instrument references.
///
/// The rack routes by index so no `String` reaches the audio thread. The
/// mapping is rebuilt whenever the document changes, and it is derived from
/// the DOCUMENT rather than from enumeration order — a keyboard that
/// re-enumerates in a different position must not change what it plays.
pub fn index_ports(document: &[InstrumentState]) -> Vec<String> {
    let mut ports: Vec<String> = Vec::new();
    for state in document {
        if let Some(name) = &state.port
            && !ports.iter().any(|p| p == name)
        {
            ports.push(name.clone());
        }
    }
    ports
}

/// Build the port report from what the document wants and what the system
/// offers. Pure so the two implementations cannot disagree about it.
pub fn report_ports(available: &[String], wanted: &[&str]) -> Vec<MidiPortReport> {
    let mut out: Vec<MidiPortReport> = available
        .iter()
        .map(|name| MidiPortReport {
            id: name.clone(),
            name: name.clone(),
            connected: wanted.contains(&name.as_str()),
            absent: false,
        })
        .collect();
    // A port an instrument names but the system does not offer still gets a
    // row: "not connected" has to be visible somewhere, or a silent
    // instrument has no explanation.
    for name in wanted {
        if !available.iter().any(|a| a == name) {
            out.push(MidiPortReport {
                id: (*name).to_owned(),
                name: (*name).to_owned(),
                connected: false,
                absent: true,
            });
        }
    }
    out
}

#[cfg(feature = "hardware")]
mod imp {
    use super::*;
    use midir::{MidiInput, MidiInputConnection};
    use std::sync::{Arc, Mutex};

    /// The shared writer. The midir callback runs on its own thread per
    /// port, so the producer is behind a mutex — contended only between
    /// port threads, never with the audio thread, which owns the consumer.
    type Writer = Arc<Mutex<Producer<MidiEvent>>>;

    pub struct Ports {
        writer: Writer,
        available: Vec<String>,
        /// Live connections, parallel to the indices `index_ports` assigns.
        connections: Vec<(String, Option<MidiInputConnection<()>>)>,
    }

    impl Ports {
        pub fn new(tx: Producer<MidiEvent>) -> Self {
            let mut ports = Ports {
                writer: Arc::new(Mutex::new(tx)),
                available: Vec::new(),
                connections: Vec::new(),
            };
            ports.refresh();
            ports
        }

        pub fn refresh(&mut self) {
            self.available = enumerate();
        }

        pub fn index_of(&self, name: Option<&str>) -> Option<u8> {
            let name = name?;
            self.connections
                .iter()
                .position(|(port, _)| port == name)
                .and_then(|i| u8::try_from(i).ok())
        }

        /// Open what the document asks for, close what it no longer does.
        pub fn bind(&mut self, document: &[InstrumentState]) {
            let wanted = index_ports(document);
            // Dropping a connection is what closes it, so rebuilding the
            // vector in document order both re-indexes and tidies up.
            let mut next: Vec<(String, Option<MidiInputConnection<()>>)> =
                Vec::with_capacity(wanted.len());
            for (index, name) in wanted.iter().enumerate() {
                let existing = self
                    .connections
                    .iter()
                    .position(|(port, conn)| port == name && conn.is_some());
                if let Some(pos) = existing
                    && self.connections[pos].0 == *name
                {
                    // Keep the live connection, but it may need a new index.
                    let (port, conn) = self.connections.remove(pos);
                    if self.connections.len() >= pos {
                        next.push((port, conn));
                        continue;
                    }
                }
                let index = u8::try_from(index).unwrap_or(u8::MAX);
                next.push((name.clone(), self.connect(name, index)));
            }
            self.connections = next;
        }

        fn connect(&self, name: &str, index: u8) -> Option<MidiInputConnection<()>> {
            let input = MidiInput::new("tributary").ok()?;
            let port = input
                .ports()
                .into_iter()
                .find(|p| input.port_name(p).is_ok_and(|n| n == name))?;
            let writer = Arc::clone(&self.writer);
            match input.connect(
                &port,
                "tributary-in",
                move |_stamp, bytes, ()| {
                    let Some(event) = crate::midi_ports::parse(index, bytes) else {
                        return;
                    };
                    let event = crate::midi_ports::normalize(event);
                    if let Ok(mut writer) = writer.lock() {
                        // A full ring means the audio thread stalled. The
                        // rack answers a drop with all-notes-off, so a lost
                        // note-off cannot hang a note forever.
                        let _ = writer.push(event);
                    }
                },
                (),
            ) {
                Ok(conn) => Some(conn),
                Err(err) => {
                    tracing::warn!(port = name, error = %err, "MIDI port would not open");
                    None
                }
            }
        }

        pub fn report(&self, wanted: &[&str]) -> Vec<MidiPortReport> {
            report_ports(&self.available, wanted)
        }

        pub fn test_note(&self, instrument: u32) {
            // Addressed at the instrument's own port index so the note goes
            // exactly where a key press would.
            let Ok(index) = u8::try_from(instrument) else {
                return;
            };
            self.push(MidiEvent {
                port: index,
                channel: 0,
                status: 0x90,
                data1: TEST_KEY,
                data2: TEST_VELOCITY,
            });
        }

        pub fn panic(&self) {
            for index in 0..self.connections.len() {
                let Ok(port) = u8::try_from(index) else {
                    continue;
                };
                for channel in 0..16 {
                    // CC 123: all notes off.
                    self.push(MidiEvent {
                        port,
                        channel,
                        status: 0xB0,
                        data1: 123,
                        data2: 0,
                    });
                }
            }
        }

        fn push(&self, event: MidiEvent) {
            if let Ok(mut writer) = self.writer.lock() {
                let _ = writer.push(event);
            }
        }
    }

    fn enumerate() -> Vec<String> {
        let Ok(input) = MidiInput::new("tributary-enumerate") else {
            tracing::warn!("no ALSA sequencer: MIDI input is unavailable");
            return Vec::new();
        };
        let mut names: Vec<String> = input
            .ports()
            .iter()
            .filter_map(|p| input.port_name(p).ok())
            // Our own input ports would show up as loopback candidates.
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

    /// No ALSA, no ports. Instruments still render — they simply have
    /// nothing playing them but the test button and the tests.
    pub struct Ports {
        writer: Option<Producer<MidiEvent>>,
        bound: Vec<String>,
    }

    impl Ports {
        pub fn new(tx: Producer<MidiEvent>) -> Self {
            Ports {
                writer: Some(tx),
                bound: Vec::new(),
            }
        }

        pub fn refresh(&mut self) {}

        pub fn index_of(&self, name: Option<&str>) -> Option<u8> {
            let name = name?;
            self.bound
                .iter()
                .position(|p| p == name)
                .and_then(|i| u8::try_from(i).ok())
        }

        pub fn bind(&mut self, document: &[InstrumentState]) {
            self.bound = index_ports(document);
        }

        pub fn report(&self, wanted: &[&str]) -> Vec<MidiPortReport> {
            report_ports(&[], wanted)
        }

        pub fn test_note(&self, instrument: u32) {
            let Ok(port) = u8::try_from(instrument) else {
                return;
            };
            self.push(MidiEvent {
                port,
                channel: 0,
                status: 0x90,
                data1: TEST_KEY,
                data2: TEST_VELOCITY,
            });
        }

        pub fn panic(&self) {}

        fn push(&self, _event: MidiEvent) {
            // The stub holds the producer so the ring stays owned, but a
            // build without hardware has no thread to write from.
            let _ = &self.writer;
        }
    }
}

pub use imp::Ports;

#[cfg(test)]
mod tests {
    use super::*;
    use trib_core::InstrumentId;

    fn instrument(id: u32, port: Option<&str>) -> InstrumentState {
        let mut state = InstrumentState::new(InstrumentId(id), format!("Inst {id}"));
        state.port = port.map(str::to_owned);
        state
    }

    #[test]
    fn port_indices_come_from_the_document_not_from_enumeration_order() {
        // A keyboard that re-enumerates in a different position must keep
        // playing the instrument it was bound to.
        let document = [
            instrument(0, Some("nanoKEY2 MIDI 1")),
            instrument(1, Some("UM-ONE MIDI 1")),
            instrument(2, Some("nanoKEY2 MIDI 1")),
        ];
        assert_eq!(
            index_ports(&document),
            ["nanoKEY2 MIDI 1", "UM-ONE MIDI 1"],
            "one index per distinct port, in first-mention order"
        );
    }

    #[test]
    fn an_instrument_with_no_port_contributes_no_index() {
        assert!(index_ports(&[instrument(0, None)]).is_empty());
    }

    #[test]
    fn a_port_an_instrument_wants_but_the_system_lacks_is_reported_absent() {
        // The whole point: a silent instrument must be explainable, and
        // "the keyboard you named is not here" is the explanation.
        let report = report_ports(&["UM-ONE MIDI 1".into()], &["nanoKEY2 MIDI 1"]);
        let absent: Vec<&MidiPortReport> = report.iter().filter(|p| p.absent).collect();
        assert_eq!(absent.len(), 1);
        assert_eq!(absent[0].name, "nanoKEY2 MIDI 1");
    }

    #[test]
    fn an_available_port_nothing_uses_is_listed_but_not_connected() {
        let report = report_ports(&["UM-ONE MIDI 1".into()], &[]);
        assert_eq!(report.len(), 1);
        assert!(!report[0].connected);
        assert!(!report[0].absent);
    }

    #[test]
    fn an_available_port_an_instrument_uses_reads_connected() {
        let report = report_ports(&["UM-ONE MIDI 1".into()], &["UM-ONE MIDI 1"]);
        assert!(report[0].connected);
        assert!(!report[0].absent);
    }
}
