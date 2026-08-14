//! Recording taps. The audio thread pushes samples into per-track rings;
//! a writer thread (trib-project) drains them to WAV. Overruns drop whole
//! samples and count them — the writer inserts silence so every track of a
//! take stays sample-aligned.

use std::sync::Arc;

use std::sync::atomic::{AtomicU64, Ordering};
use trib_core::CapturedMidi;

use rtrb::Producer;

/// Ring seconds per recorded track: generous slack for a stalled writer
/// (a busy disk) before anything is lost. ~1.5 MB per mono track at 48 kHz.
pub const RECORD_RING_SECS: usize = 8;

/// Events a take's MIDI capture can hold before the writer drains it.
///
/// The writer wakes every 50 ms; sixteen thousand events is minutes of
/// dense playing, so this only ever fills if the writer thread has died —
/// and a lost note is the right thing to lose there, because the audio is
/// what the room heard.
pub const MIDI_CAPTURE_CAPACITY: usize = 16_384;

pub struct RecordTrack {
    pub tx: Producer<f32>,
    /// Samples dropped on ring overrun; the writer replaces them with
    /// silence and reports the count in the take manifest.
    pub dropped: Arc<AtomicU64>,
}

impl RecordTrack {
    #[inline]
    pub fn push(&mut self, sample: f32) {
        if self.tx.push(sample).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Everything the audio thread needs to tap a take. Built control-side
/// (against the CURRENT compile's strip order — topology changes are
/// refused while recording), moved in via `StartRecord`, returned through
/// the retire ring on stop so producers drop control-side.
pub struct RecordSet {
    /// Strip position (compile order) → index into `tracks`. Strips tap
    /// post-gain post-EQ, pre-fader: dry takes, the live-capture norm.
    pub strip_tracks: Vec<Option<u16>>,
    /// The stereo mix (post-master-fader), interleaved L/R.
    pub master_track: Option<u16>,
    pub tracks: Vec<RecordTrack>,
    /// The notes, beside the audio.
    ///
    /// Carried inside the record set rather than installed by a second
    /// command, so the sidecar starts and stops on exactly the block the
    /// audio does. Two commands could land in different blocks, and the
    /// take writer exists to guarantee they do not.
    pub midi: Option<Box<MidiCapture>>,
}

/// The MIDI half of a take. Filled by the rack as it applies events, drained
/// by the take writer.
pub struct MidiCapture {
    pub tx: Producer<CapturedMidi>,
    pub dropped: Arc<AtomicU64>,
    /// Engine sample position when recording started, so stamps come out
    /// relative to the take rather than to the daemon's uptime.
    pub start_sample: u64,
}

impl MidiCapture {
    /// Record one applied event. A full ring loses the note rather than
    /// the take: the audio is authoritative and must not stall for the
    /// sidecar.
    #[inline]
    pub fn push(&mut self, event: CapturedMidi) {
        if self.tx.push(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
