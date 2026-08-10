//! The control↔audio wire: commands in, fixed-size meter blocks out,
//! retired graphs back. Nothing crossing this boundary allocates or locks
//! on the audio side; boxes are built and dropped control-side only.

use trib_dsp::Coefficients;

use crate::compiled::CompiledGraph;
use crate::playback::PlaybackSet;
use crate::record::RecordSet;

/// Index into the compiled graph's parameter dispatch table. Resolved
/// control-side at compile time; the audio thread never hashes or searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamIx(pub u16);

/// Index into the boolean-flag dispatch table (PFL, EQ engage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlagIx(pub u16);

/// Index into the EQ-band dispatch table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EqIx(pub u16);

/// Control → audio. All dB→linear and coefficient math happens
/// control-side; the audio thread only installs values.
pub enum EngineCommand {
    /// Smoothed continuous parameter (linear gain, pan position, mute 0/1).
    SetParam {
        param: ParamIx,
        target: f32,
    },
    SetFlag {
        flag: FlagIx,
        on: bool,
    },
    SetEqCoeffs {
        eq: EqIx,
        coeffs: Coefficients<f32>,
    },
    /// Topology change: a freshly compiled graph, built control-side. The
    /// audio thread installs it at a block boundary and retires the old one.
    SwapGraph {
        graph: Box<CompiledGraph>,
    },
    /// Begin tapping a take. Built control-side; the audio thread only
    /// moves it into place.
    StartRecord {
        set: Box<RecordSet>,
    },
    /// End the take: the set goes back through the retire ring, its
    /// producers drop control-side, and the writer finalizes.
    StopRecord,
    /// Begin the tape return. Built control-side with pre-primed rings; the
    /// audio thread only moves it into place.
    StartPlayback {
        set: Box<PlaybackSet>,
    },
    /// End playback (stop, or a seek's teardown half): the set retires so
    /// its consumers drop control-side, which aborts the feeder.
    StopPlayback,
    /// Playback-lane gates. Stale track indices after a session swap are
    /// ignored, not errors.
    SetPlaybackSolo {
        track: u16,
        on: bool,
    },
    SetPlaybackMute {
        track: u16,
        on: bool,
    },
    /// Where the playback mix goes: the hardware output (replacing the live
    /// mix) or the browser monitor ring (live mix stays on the wire).
    SetMonitorTarget {
        target: MonitorTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MonitorTarget {
    #[default]
    Hardware,
    Stream,
}

/// Monitor ring capacity: ~2 s of interleaved stereo at 48 kHz. The pump
/// drains every 50 ms; a full ring drops samples (browser review audio is
/// droppable, like meters).
pub const MONITOR_RING_CAPACITY: usize = 96_000 * 2;

/// Anything the audio thread must hand back for control-side dropping.
pub enum Retired {
    Graph(Box<CompiledGraph>),
    Record(Box<RecordSet>),
    Playback(Box<PlaybackSet>),
}

/// Command ring depth. Far above any real gesture rate; a full ring means
/// the audio thread stalled, and the control side logs loudly rather than
/// waiting.
pub const CMD_RING_CAPACITY: usize = 1024;

/// Metered points per block: strips (≤32) + buses + FX + master fit with
/// room to spare.
pub const MAX_METERS: usize = 64;

/// Telemetry ring depth: ~64 blocks ≈ 340 ms at 48 kHz / 256. Meters are
/// droppable; a full ring skips the push.
pub const METER_RING_CAPACITY: usize = 64;

/// Retired-graph ring: the pump drains it every 50 ms, so even a burst of
/// topology edits can't realistically fill it.
pub const RETIRE_RING_CAPACITY: usize = 16;

/// One block's worth of peaks, pushed audio→control once per callback.
/// Fixed-size and `Copy`: the audio thread never allocates for telemetry.
#[derive(Debug, Clone, Copy)]
pub struct MeterBlock {
    /// Monotone block counter — lets the pump detect gaps if it cares.
    pub frame: u64,
    /// How many entries of `peak`/`clip_bits` are live.
    pub count: u8,
    /// Linear peak per metered point.
    pub peak: [f32; MAX_METERS],
    /// Bit i set = point i saw a clipped sample this block.
    pub clip_bits: u64,
}

impl MeterBlock {
    pub fn empty(frame: u64) -> Self {
        MeterBlock {
            frame,
            count: 0,
            peak: [0.0; MAX_METERS],
            clip_bits: 0,
        }
    }

    pub fn clipped(&self, index: usize) -> bool {
        self.clip_bits & (1 << index) != 0
    }
}
