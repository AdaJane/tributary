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

    fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("take.mid");
        std::fs::write(&path, bytes).expect("write");
        (dir, path)
    }

    #[test]
    fn a_written_sidecar_parses_back_to_the_events_that_were_captured() {
        // The round trip that justifies using a crate to READ. Pairing our
        // own reader with our own writer would only ever have round-tripped
        // our own bugs; parsing with an independent implementation turns
        // the byte-exact test above into a genuine conformance check.
        const SR: u32 = 48_000;
        let captured = vec![
            note(0, 0, 0x90, 60, 100),
            note(SR as u64 / 2, 0, 0x80, 60, 0),
            note(SR as u64, 0, 0xB0, 64, 127),
            note(SR as u64 * 2, 0, 0xC0, 4, 0),
        ];
        let bytes = build_smf("Rhodes", SR, &captured);
        let (_dir, path) = write_temp(&bytes);
        let back = read_midi_sidecar(&path, SR).expect("our own file parses");

        assert_eq!(back.len(), captured.len());
        for (read, wrote) in back.iter().zip(&captured) {
            // The writer re-addresses everything to channel 1 (wire 0), so
            // the status comes back with a zero channel nibble.
            assert_eq!(read.status, wrote.status & 0xF0);
            assert_eq!(read.data1, wrote.data1);
            // A tick is 1/960 s, so a sample can land one tick either way.
            let tolerance = SR as u64 / TICKS_PER_SECOND + 1;
            assert!(
                read.sample.abs_diff(wrote.sample) <= tolerance,
                "{} vs {}",
                read.sample,
                wrote.sample
            );
        }
        assert_eq!(back[3].data2, 0, "a program change carries two bytes");
    }

    #[test]
    fn ticks_map_back_to_the_samples_they_came_from_at_a_non_integral_rate() {
        // 44.1 kHz is where accumulating rounded deltas would drift; both
        // directions compute from the ABSOLUTE position for that reason.
        const SR: u32 = 44_100;
        for seconds in [0u64, 1, 7, 60, 600] {
            let sample = seconds * u64::from(SR);
            let tick = tick_of(sample, SR);
            let back = sample_of(tick, MICROS_PER_QUARTER, TICKS_PER_QUARTER, SR);
            assert!(
                back.abs_diff(sample) <= u64::from(SR) / TICKS_PER_SECOND + 1,
                "{seconds}s: {sample} -> {tick} -> {back}"
            );
        }
    }

    #[test]
    fn a_file_at_another_tempo_maps_to_the_right_samples() {
        // Proves the reader uses the FILE's division and tempo rather than
        // this module's constants — a sidecar copied in from another
        // program is not obliged to use ours.
        const SR: u32 = 48_000;
        // 96 ticks/quarter at 1_000_000 µs/quarter = 96 ticks per second.
        let mut track: Vec<u8> = Vec::new();
        write_vlq(&mut track, 0);
        track.extend_from_slice(&[0xFF, 0x51, 0x03]);
        track.extend_from_slice(&1_000_000u32.to_be_bytes()[1..]);
        write_vlq(&mut track, 96); // one second in
        track.extend_from_slice(&[0x90, 60, 100]);
        write_vlq(&mut track, 0);
        track.extend_from_slice(&[0xFF, 0x2F, 0x00]);
        let mut header = Vec::new();
        header.extend_from_slice(&0u16.to_be_bytes());
        header.extend_from_slice(&1u16.to_be_bytes());
        header.extend_from_slice(&96u16.to_be_bytes());
        let mut bytes = chunk(b"MThd", &header);
        bytes.extend(chunk(b"MTrk", &track));

        let (_dir, path) = write_temp(&bytes);
        let events = read_midi_sidecar(&path, SR).expect("parses");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sample, u64::from(SR), "exactly one second in");
    }

    #[test]
    fn a_truncated_file_never_panics_and_a_headless_one_is_refused() {
        // The guarantee reading through a crate buys: a bad length prefix
        // hand-rolled is a panic or an OOM. Every cut below must produce an
        // answer rather than a crash.
        let bytes = build_smf("Rhodes", 48_000, &[note(0, 0, 0x90, 60, 100)]);
        for cut in 0..bytes.len() {
            let (_dir, path) = write_temp(&bytes[..cut]);
            let _ = read_midi_sidecar(&path, 48_000);
        }

        // Cut short of any track chunk, the file is structurally empty.
        // midly parses that happily as zero tracks, and answering "no
        // notes" would hide a truncated take behind a silent playback.
        let (_dir, path) = write_temp(&bytes[..14]);
        assert!(matches!(
            read_midi_sidecar(&path, 48_000),
            Err(MidiReadError::Parse(_))
        ));

        // But a file one byte short still yields its notes: the trailing
        // end-of-track marker is not what the notes are stored in, and
        // refusing a recoverable file would lose a take's MIDI for nothing.
        let (_dir, path) = write_temp(&bytes[..bytes.len() - 1]);
        assert_eq!(
            read_midi_sidecar(&path, 48_000).expect("recoverable").len(),
            1
        );
    }

    #[test]
    fn a_missing_sidecar_is_an_io_error_rather_than_an_empty_take() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_midi_sidecar(&dir.path().join("nope.mid"), 48_000),
            Err(MidiReadError::Io(_))
        ));
    }

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

/// One event of a sidecar, at its position in the take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidecarEvent {
    /// Samples since the take started — the same clock
    /// [`trib_core::CapturedMidi::sample`] uses, so a feeder and the
    /// playhead speak one unit.
    pub sample: u64,
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum MidiReadError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a MIDI file this daemon can read: {0}")]
    Parse(String),
}

/// Tick → sample position, at a stated tempo and division.
///
/// The inverse of [`tick_of`] and written the same way: from the absolute
/// tick every time, `u128` intermediate, never by accumulating rounded
/// deltas.
fn sample_of(tick: u64, micros_per_quarter: u32, ticks_per_quarter: u16, sample_rate: u32) -> u64 {
    let ticks = u128::from(ticks_per_quarter.max(1));
    (u128::from(tick) * u128::from(micros_per_quarter) * u128::from(sample_rate)
        / (ticks * 1_000_000)) as u64
}

/// Read one MIDI sidecar into take order.
///
/// Named for its format because `peaks` owns a `read_sidecar` too — the
/// take carries both kinds, and this crate re-exports flat.
///
/// Liberal on the way in, per the robustness rule: every track of a
/// format-0 or format-1 file is merged, running status is accepted (midly
/// handles it), and the file's OWN division and tempo decide the sample
/// mapping rather than this module's constants — a file another program
/// wrote is not obliged to use ours.
///
/// Meta and SysEx events are dropped: the daemon's own MIDI path discards
/// everything `>= 0xF0` at the port (`midi_ports::parse`), so passing them
/// on here would send an external synth something the internal one could
/// never have received.
pub fn read_midi_sidecar(
    path: &Path,
    sample_rate: u32,
) -> Result<Vec<SidecarEvent>, MidiReadError> {
    let bytes = std::fs::read(path)?;
    let smf = midly::Smf::parse(&bytes).map_err(|e| MidiReadError::Parse(e.to_string()))?;
    let ticks_per_quarter = match smf.header.timing {
        midly::Timing::Metrical(ticks) => ticks.as_int(),
        // SMPTE division states ticks per FRAME. Refused rather than
        // guessed at: silently treating it as metrical would play the file
        // at an arbitrary wrong speed, which is worse than not playing it.
        midly::Timing::Timecode(..) => {
            return Err(MidiReadError::Parse(
                "SMPTE-timed files are not supported".into(),
            ));
        }
    };

    // A header with no track chunk parses cleanly and carries nothing.
    // The writer never produces one — a voice nobody played writes no file
    // at all — so in practice this is a truncated file, and answering
    // "no notes" would hide that behind a silent playback.
    if smf.tracks.is_empty() {
        return Err(MidiReadError::Parse("the file has no tracks".into()));
    }

    let mut out: Vec<SidecarEvent> = Vec::new();
    for track in &smf.tracks {
        // Tempo is per-track in the file's own order, and a format-1 file
        // conventionally puts it in track 0 — so each track walks its own
        // tempo, starting from the MIDI default.
        let mut micros_per_quarter = MICROS_PER_QUARTER;
        let mut tick = 0u64;
        for event in track {
            tick += u64::from(event.delta.as_int());
            match event.kind {
                midly::TrackEventKind::Meta(midly::MetaMessage::Tempo(micros)) => {
                    micros_per_quarter = micros.as_int();
                }
                midly::TrackEventKind::Midi { channel, message } => {
                    let (status, data1, data2) = encode(message);
                    out.push(SidecarEvent {
                        sample: sample_of(tick, micros_per_quarter, ticks_per_quarter, sample_rate),
                        status: status | channel.as_int(),
                        data1,
                        data2,
                    });
                }
                _ => {}
            }
        }
    }
    out.sort_by_key(|event| event.sample);
    Ok(out)
}

/// One channel-voice message as its three wire bytes.
fn encode(message: midly::MidiMessage) -> (u8, u8, u8) {
    use midly::MidiMessage::*;
    match message {
        NoteOff { key, vel } => (0x80, key.as_int(), vel.as_int()),
        NoteOn { key, vel } => (0x90, key.as_int(), vel.as_int()),
        Aftertouch { key, vel } => (0xA0, key.as_int(), vel.as_int()),
        Controller { controller, value } => (0xB0, controller.as_int(), value.as_int()),
        ProgramChange { program } => (0xC0, program.as_int(), 0),
        ChannelAftertouch { vel } => (0xD0, vel.as_int(), 0),
        PitchBend { bend } => {
            let value = bend.0.as_int();
            (0xE0, (value & 0x7F) as u8, (value >> 7) as u8)
        }
    }
}
