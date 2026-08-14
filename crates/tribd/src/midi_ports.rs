//! MIDI input ports: enumeration, on-demand connection, and the parse that
//! turns raw bytes into the fixed event the rack understands.
//!
//! Doctrine copied from the device orchestrator, machinery deliberately not:
//! ports open when something wants them, close when nothing does, and are
//! re-enumerated only when asked. There is no background retry, and a port
//! whose name changed is reported absent rather than guessed at — the same
//! rule that stops a patch adopting the wrong microphone.
//!
//! The parse is here rather than in the rack because it is the one part
//! that is pure: given bytes, which event (if any) crosses to the audio
//! thread.

use trib_engine::MidiEvent;

/// Turn one raw MIDI message into an event for `port`.
///
/// Channel-voice messages only. System messages (`0xF0` and up — SysEx,
/// clock, active sensing, reset) are dropped: none of them reaches a
/// SoundFont voice, and active sensing in particular arrives every 300 ms
/// from many keyboards and would be pure ring traffic.
///
/// Running status is not reconstructed. ALSA sequencer sources deliver
/// whole messages, so a status-less buffer here means a malformed one.
pub fn parse(port: u8, bytes: &[u8]) -> Option<MidiEvent> {
    let status = *bytes.first()?;
    if !(0x80..0xF0).contains(&status) {
        return None;
    }
    let kind = status & 0xF0;
    let channel = status & 0x0F;
    // Program change and channel pressure carry one data byte; everything
    // else we act on carries two.
    let wanted = match kind {
        0xC0 | 0xD0 => 2,
        _ => 3,
    };
    if bytes.len() < wanted {
        return None;
    }
    Some(MidiEvent {
        port,
        channel,
        status: kind,
        data1: bytes[1] & 0x7F,
        data2: if wanted == 3 { bytes[2] & 0x7F } else { 0 },
    })
}

/// A note-on with velocity 0 is a note-off — the convention every keyboard
/// that uses running status relies on. Normalising here means the rack and
/// the capture both see one representation of "key released".
pub fn normalize(event: MidiEvent) -> MidiEvent {
    if event.status == 0x90 && event.data2 == 0 {
        MidiEvent {
            status: 0x80,
            ..event
        }
    } else {
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_on_parses_with_its_channel_split_off_the_status() {
        let event = parse(3, &[0x95, 60, 100]).unwrap();
        assert_eq!(event.port, 3);
        assert_eq!(event.channel, 5);
        assert_eq!(event.status, 0x90);
        assert_eq!(event.data1, 60);
        assert_eq!(event.data2, 100);
    }

    #[test]
    fn a_program_change_parses_from_two_bytes() {
        let event = parse(0, &[0xC0, 4]).unwrap();
        assert_eq!(event.status, 0xC0);
        assert_eq!(event.data1, 4);
        assert_eq!(event.data2, 0);
    }

    #[test]
    fn system_messages_never_reach_the_audio_thread() {
        // Active sensing arrives every 300 ms from many keyboards; clock
        // arrives 24 times a beat. Neither can move a SoundFont voice, and
        // both would be pure ring traffic.
        assert!(parse(0, &[0xFE]).is_none());
        assert!(parse(0, &[0xF8]).is_none());
        assert!(parse(0, &[0xF0, 0x7E, 0xF7]).is_none());
    }

    #[test]
    fn a_truncated_message_is_dropped_rather_than_guessed() {
        assert!(parse(0, &[0x90, 60]).is_none());
        assert!(parse(0, &[]).is_none());
        // A data byte with no status is running status, which the ALSA
        // sequencer never hands us — so it is malformed, not a shortcut.
        assert!(parse(0, &[60, 100]).is_none());
    }

    #[test]
    fn a_note_on_at_velocity_zero_is_a_note_off() {
        let event = normalize(parse(0, &[0x90, 60, 0]).unwrap());
        assert_eq!(event.status, 0x80);
        assert_eq!(event.data1, 60);
    }

    #[test]
    fn a_real_note_on_survives_normalization() {
        let event = normalize(parse(0, &[0x90, 60, 1]).unwrap());
        assert_eq!(event.status, 0x90);
        assert_eq!(event.data2, 1);
    }
}
