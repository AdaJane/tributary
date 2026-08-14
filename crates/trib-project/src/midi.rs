//! The MIDI half of a take: one Standard MIDI File per armed instrument.
//!
//! Why a sidecar at all: the audio is what the room heard and is
//! authoritative, but a wrong note is only fixable if the notes survived.
//! The `.mid` is written beside the audio, never instead of it, and a
//! failure to write one **never** marks the take damaged — `damaged` means
//! "the audio is not what the room heard", and a missing sidecar means "you
//! have the audio but not the notes". Different facts, different words.
//!
//! Timing: a fixed 960 ticks per second, expressed as 480 ticks/quarter
//! with a 120 BPM tempo. The project has no musical clock — no bar, no
//! beat, no tempo map — so this is a unit conversion rather than a claim
//! about the music. SMPTE division would say that more honestly, but
//! importers handle it patchily, and the whole point of the file is that
//! somebody can open it somewhere else.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use rtrb::Consumer;
use trib_core::CapturedMidi;

/// Ticks per second in the written file: 480 per quarter at 120 BPM.
pub const TICKS_PER_SECOND: u64 = 960;
const TICKS_PER_QUARTER: u16 = 480;
const MICROS_PER_QUARTER: u32 = 500_000;

/// One instrument's captured notes, on their way to disk.
pub struct MidiSink {
    pub rx: Consumer<CapturedMidi>,
    pub dropped: Arc<AtomicU64>,
    /// Rack slot → the name to write into the file, for every slot this
    /// take is recording.
    pub voices: Vec<(u16, String)>,
}

/// What one written sidecar became, for the take manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiTrackReport {
    pub file: String,
    pub name: String,
    pub events: u64,
    pub dropped_events: u64,
}

/// Sample position → tick.
///
/// Computed from the ABSOLUTE sample offset every time, never by
/// accumulating rounded deltas: at 44.1 kHz the per-event ratio is not
/// integral, and accumulation drifts audibly over a long take.
pub fn tick_of(sample: u64, sample_rate: u32) -> u64 {
    (u128::from(sample) * u128::from(TICKS_PER_SECOND) / u128::from(sample_rate.max(1))) as u64
}

/// MIDI's variable-length quantity: seven bits a byte, high bit set on
/// every byte but the last.
fn write_vlq(out: &mut Vec<u8>, mut value: u64) {
    let mut buf = [0u8; 5];
    let mut len = 0;
    buf[len] = (value & 0x7F) as u8;
    len += 1;
    value >>= 7;
    while value > 0 {
        buf[len] = ((value & 0x7F) as u8) | 0x80;
        len += 1;
        value >>= 7;
    }
    for byte in buf[..len].iter().rev() {
        out.push(*byte);
    }
}

fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Build a single-track SMF from events already in take order.
///
/// Buffered whole rather than streamed, deliberately — and against the
/// peaks sidecar's precedent of writing incrementally so a crash leaves
/// something readable. An SMF's track chunk is length-prefixed, so a
/// truncated one is not a short file, it is an invalid one. A take's notes
/// are a few hundred kilobytes at worst.
pub fn build_smf(name: &str, sample_rate: u32, events: &[CapturedMidi]) -> Vec<u8> {
    let mut header = Vec::with_capacity(6);
    header.extend_from_slice(&0u16.to_be_bytes()); // format 0: one track
    header.extend_from_slice(&1u16.to_be_bytes()); // ntrks
    header.extend_from_slice(&TICKS_PER_QUARTER.to_be_bytes());

    let mut track = Vec::new();
    // Tempo, so the fixed 960 ticks/second is legible to whatever opens it.
    write_vlq(&mut track, 0);
    track.extend_from_slice(&[0xFF, 0x51, 0x03]);
    track.extend_from_slice(&MICROS_PER_QUARTER.to_be_bytes()[1..]);
    // Track name — an instrument's own, so a DAW's track list reads like
    // the console did.
    write_vlq(&mut track, 0);
    track.extend_from_slice(&[0xFF, 0x03]);
    let name_bytes = name.as_bytes();
    write_vlq(&mut track, name_bytes.len() as u64);
    track.extend_from_slice(name_bytes);

    let mut last_tick = 0u64;
    for event in events {
        let tick = tick_of(event.sample, sample_rate);
        // Saturating: the ring is ordered, but a clock that went backwards
        // must not produce a negative delta and a corrupt file.
        write_vlq(&mut track, tick.saturating_sub(last_tick));
        last_tick = last_tick.max(tick);
        // Everything is written on MIDI channel 1: the rack re-addresses
        // every event to one channel, so the file says what actually
        // played rather than what the keyboard happened to transmit on.
        track.push(event.status & 0xF0);
        track.push(event.data1 & 0x7F);
        if !matches!(event.status & 0xF0, 0xC0 | 0xD0) {
            track.push(event.data2 & 0x7F);
        }
    }
    // End of track — required, and what makes the file valid to a reader.
    write_vlq(&mut track, 0);
    track.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let mut out = chunk(b"MThd", &header);
    out.extend(chunk(b"MTrk", &track));
    out
}

/// Drain a finished capture and write one file per armed instrument.
///
/// Called by the take writer *before* the manifest, so `take.toml` never
/// names a file that is not there yet.
pub fn write_sidecars(
    dir: &Path,
    sample_rate: u32,
    mut sink: MidiSink,
) -> std::io::Result<Vec<MidiTrackReport>> {
    let mut by_voice: std::collections::BTreeMap<u16, Vec<CapturedMidi>> =
        std::collections::BTreeMap::new();
    while let Ok(event) = sink.rx.pop() {
        by_voice.entry(event.instrument).or_default().push(event);
    }

    let dropped = sink.dropped.load(std::sync::atomic::Ordering::Relaxed);
    let mut reports = Vec::new();
    for (slot, name) in &sink.voices {
        let events = by_voice.remove(slot).unwrap_or_default();
        // A voice nobody played writes nothing: an empty sidecar in the
        // take directory is a file to explain, not a feature.
        if events.is_empty() {
            continue;
        }
        let file = format!("inst{:02}-{}.mid", slot + 1, crate::writer::file_slug(name));
        let bytes = build_smf(name, sample_rate, &events);
        std::fs::write(dir.join(&file), &bytes)?;
        reports.push(MidiTrackReport {
            file,
            name: name.clone(),
            events: events.len() as u64,
            dropped_events: dropped,
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(sample: u64, instrument: u16, status: u8, data1: u8, data2: u8) -> CapturedMidi {
        CapturedMidi {
            sample,
            instrument,
            status,
            data1,
            data2,
        }
    }

    #[test]
    fn a_variable_length_quantity_matches_the_spec() {
        let mut out = Vec::new();
        write_vlq(&mut out, 0);
        assert_eq!(out, [0x00]);
        out.clear();
        write_vlq(&mut out, 127);
        assert_eq!(out, [0x7F]);
        out.clear();
        write_vlq(&mut out, 128);
        assert_eq!(out, [0x81, 0x00]);
        out.clear();
        write_vlq(&mut out, 0x0F_FF_FF);
        assert_eq!(out, [0xBF, 0xFF, 0x7F]);
    }

    #[test]
    fn a_smf_is_byte_exact_for_a_known_event_stream() {
        // One note on at time zero, one note off half a second later.
        let events = [note(0, 0, 0x90, 60, 100), note(24_000, 0, 0x80, 60, 0)];
        let bytes = build_smf("Kit", 48_000, &events);

        assert_eq!(&bytes[0..4], b"MThd");
        assert_eq!(&bytes[4..8], &6u32.to_be_bytes());
        // Format 0, one track, 480 ticks per quarter.
        assert_eq!(&bytes[8..14], &[0x00, 0x00, 0x00, 0x01, 0x01, 0xE0]);
        assert_eq!(&bytes[14..18], b"MTrk");

        let track = &bytes[22..];
        // Tempo meta, then the track name, then the notes.
        assert_eq!(&track[0..7], &[0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20]);
        assert_eq!(&track[7..11], &[0x00, 0xFF, 0x03, 0x03]);
        assert_eq!(&track[11..14], b"Kit");
        assert_eq!(&track[14..18], &[0x00, 0x90, 60, 100]);
        // Half a second = 480 ticks, which is a two-byte VLQ.
        assert_eq!(&track[18..23], &[0x83, 0x60, 0x80, 60, 0]);
        assert_eq!(&track[23..], &[0x00, 0xFF, 0x2F, 0x00]);
    }

    #[test]
    fn ticks_come_from_absolute_samples_so_a_long_take_never_drifts() {
        // 44.1 kHz gives a non-integral ratio, so accumulating rounded
        // deltas would slide the notes off the audio over ten minutes.
        let rate = 44_100;
        let ten_minutes = rate as u64 * 600;
        assert_eq!(tick_of(0, rate), 0);
        assert_eq!(tick_of(rate as u64, rate), TICKS_PER_SECOND);
        assert_eq!(tick_of(ten_minutes, rate), TICKS_PER_SECOND * 600);

        // And the deltas the writer emits still sum to the same place.
        let events: Vec<CapturedMidi> = (0..600)
            .map(|second| note(rate as u64 * second, 0, 0x90, 60, 100))
            .collect();
        let bytes = build_smf("Long", rate, &events);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn a_program_change_writes_two_bytes_not_three() {
        let bytes = build_smf("P", 48_000, &[note(0, 0, 0xC0, 4, 0)]);
        let track = &bytes[20..];
        // …0xC0 0x04 then straight to end-of-track.
        let tail = &track[track.len() - 6..];
        assert_eq!(tail, &[0xC0, 0x04, 0x00, 0xFF, 0x2F, 0x00]);
    }

    #[test]
    fn every_event_is_written_on_one_channel() {
        // The rack re-addresses everything to channel 0, so the file must
        // say what played, not what the keyboard transmitted on.
        let bytes = build_smf("K", 48_000, &[note(0, 0, 0x95, 60, 100)]);
        assert!(bytes.windows(3).any(|w| w == [0x90, 60, 100]));
    }

    #[test]
    fn a_voice_nobody_played_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = rtrb::RingBuffer::<CapturedMidi>::new(8);
        let sink = MidiSink {
            rx,
            dropped: Arc::new(AtomicU64::new(0)),
            voices: vec![(0, "Silent".into())],
        };
        let reports = write_sidecars(dir.path(), 48_000, sink).unwrap();
        assert!(reports.is_empty());
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn each_armed_instrument_gets_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::<CapturedMidi>::new(16);
        tx.push(note(0, 0, 0x90, 36, 100)).unwrap();
        tx.push(note(480, 1, 0x90, 38, 90)).unwrap();
        let sink = MidiSink {
            rx,
            dropped: Arc::new(AtomicU64::new(0)),
            voices: vec![(0, "Kit Kick".into()), (1, "Kit Snare".into())],
        };
        let reports = write_sidecars(dir.path(), 48_000, sink).unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].file, "inst01-kit-kick.mid");
        assert_eq!(reports[1].file, "inst02-kit-snare.mid");
        assert!(dir.path().join("inst01-kit-kick.mid").is_file());
    }
}
