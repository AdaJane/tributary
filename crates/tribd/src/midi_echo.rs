//! Where one incoming MIDI event must also go.
//!
//! Pure: given the routes, the rack document and one event, this decides
//! which (port, message) pairs leave the box. Split out from the port
//! plumbing for the reason `midi_ports::parse` is — it is the part with a
//! decision in it, and a decision is worth testing without a keyboard
//! plugged in.
//!
//! Nothing here touches the audio thread. An echo is a fan-out on the same
//! midir callback the event already arrived on, and a thru is the same
//! fan-out without the instrument filter.

use trib_core::{InstrumentState, MidiRoute, MidiSource};
use trib_engine::MidiEvent;

/// One message on its way out: which port, and what to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub port: String,
    pub channel: u8,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

/// Fan one incoming event out to every route that wants it.
///
/// `port_names` maps the event's port INDEX back to the name a route
/// stores — the reverse of what `index_ports` built for the rack.
///
/// [`MidiSource::Take`] routes never appear here: a sidecar is played by
/// the feeder against the playhead, not by something arriving at a
/// keyboard.
pub fn fan_out(
    routes: &[MidiRoute],
    instruments: &[InstrumentState],
    port_names: &[String],
    event: &MidiEvent,
) -> Vec<Outgoing> {
    let Some(from) = port_names.get(usize::from(event.port)) else {
        return Vec::new();
    };
    routes
        .iter()
        .filter(|route| match &route.source {
            // The SAME predicate the rack applies, stated once in the
            // document crate — an echo that disagreed with what actually
            // sounded would be worse than no echo at all.
            MidiSource::Instrument { id } => instruments
                .iter()
                .find(|i| i.id == *id)
                .is_some_and(|i| i.accepts(from, event.channel)),
            // A thru forwards the whole port, every channel: there is no
            // instrument in the middle to have an opinion.
            MidiSource::Port { name } => name == from,
            MidiSource::Take { .. } => false,
        })
        .map(|route| Outgoing {
            port: route.port.clone(),
            channel: route.channel.unwrap_or(event.channel),
            status: event.status & 0xF0,
            data1: event.data1,
            data2: event.data2,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use trib_core::InstrumentId;

    use super::*;

    fn instrument(id: u32, port: Option<&str>, channel: Option<u8>) -> InstrumentState {
        InstrumentState {
            port: port.map(str::to_owned),
            midi_channel: channel,
            ..InstrumentState::new(InstrumentId(id), format!("Inst {id}"))
        }
    }

    fn note(port: u8, channel: u8) -> MidiEvent {
        MidiEvent {
            port,
            channel,
            status: 0x90,
            data1: 60,
            data2: 100,
        }
    }

    fn echo(id: u32, to: &str) -> MidiRoute {
        MidiRoute {
            port: to.into(),
            channel: None,
            source: MidiSource::Instrument {
                id: InstrumentId(id),
            },
        }
    }

    #[test]
    fn an_echo_sends_only_what_its_instruments_binding_accepts() {
        // The bug this exists for: a keyboard bound to channel 10 echoing
        // channel 1 would put notes on an external synth that the rack
        // itself never played — an echo that disagrees with what sounded.
        let instruments = vec![instrument(0, Some("nanoKEY2"), Some(9))];
        let ports = vec!["nanoKEY2".to_owned()];
        let routes = vec![echo(0, "Juno")];

        assert_eq!(fan_out(&routes, &instruments, &ports, &note(0, 9)).len(), 1);
        assert!(fan_out(&routes, &instruments, &ports, &note(0, 0)).is_empty());
    }

    #[test]
    fn an_instrument_bound_to_no_port_echoes_nothing() {
        let instruments = vec![instrument(0, None, None)];
        let ports = vec!["nanoKEY2".to_owned()];
        assert!(fan_out(&[echo(0, "Juno")], &instruments, &ports, &note(0, 0)).is_empty());
    }

    #[test]
    fn an_omni_instrument_echoes_every_channel_of_its_own_port() {
        let instruments = vec![instrument(0, Some("nanoKEY2"), None)];
        let ports = vec!["nanoKEY2".to_owned(), "other".to_owned()];
        let routes = vec![echo(0, "Juno")];
        assert_eq!(fan_out(&routes, &instruments, &ports, &note(0, 5)).len(), 1);
        assert!(
            fan_out(&routes, &instruments, &ports, &note(1, 5)).is_empty(),
            "omni is every channel of ITS port, not every port"
        );
    }

    #[test]
    fn a_thru_route_forwards_every_channel_of_its_input_port() {
        let ports = vec!["nanoKEY2".to_owned()];
        let routes = vec![MidiRoute {
            port: "Juno".into(),
            channel: None,
            source: MidiSource::Port {
                name: "nanoKEY2".into(),
            },
        }];
        for channel in [0, 7, 15] {
            assert_eq!(fan_out(&routes, &[], &ports, &note(0, channel)).len(), 1);
        }
    }

    #[test]
    fn a_route_that_forces_a_channel_rewrites_the_message() {
        let instruments = vec![instrument(0, Some("nanoKEY2"), None)];
        let ports = vec!["nanoKEY2".to_owned()];
        let routes = vec![MidiRoute {
            channel: Some(9),
            ..echo(0, "Juno")
        }];
        let out = fan_out(&routes, &instruments, &ports, &note(0, 3));
        assert_eq!(out[0].channel, 9);
        assert_eq!(out[0].status, 0x90, "the status keeps its kind");
    }

    #[test]
    fn several_routes_may_merge_onto_one_port() {
        let instruments = vec![
            instrument(0, Some("nanoKEY2"), None),
            instrument(1, Some("nanoKEY2"), None),
        ];
        let ports = vec!["nanoKEY2".to_owned()];
        let routes = vec![echo(0, "Juno"), echo(1, "Juno")];
        assert_eq!(fan_out(&routes, &instruments, &ports, &note(0, 0)).len(), 2);
    }

    #[test]
    fn a_take_route_is_never_an_echo() {
        // A sidecar is played against the PLAYHEAD by the feeder. If it
        // leaked in here it would double every note a keyboard played.
        let routes = vec![MidiRoute {
            port: "Juno".into(),
            channel: None,
            source: MidiSource::Take { name: "Kit".into() },
        }];
        assert!(fan_out(&routes, &[], &["nanoKEY2".to_owned()], &note(0, 0)).is_empty());
    }

    #[test]
    fn an_event_from_a_port_index_nobody_mapped_goes_nowhere() {
        let instruments = vec![instrument(0, Some("nanoKEY2"), None)];
        assert!(fan_out(&[echo(0, "Juno")], &instruments, &[], &note(0, 0)).is_empty());
    }
}
