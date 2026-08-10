//! Recording taps. The audio thread pushes samples into per-track rings;
//! a writer thread (trib-project) drains them to WAV. Overruns drop whole
//! samples and count them — the writer inserts silence so every track of a
//! take stays sample-aligned.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::Producer;

/// Ring seconds per recorded track: generous slack for a stalled writer
/// (a busy disk) before anything is lost. ~1.5 MB per mono track at 48 kHz.
pub const RECORD_RING_SECS: usize = 8;

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
}
