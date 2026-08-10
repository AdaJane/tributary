//! The real-time engine: lock-free control↔audio protocol, graph compiler,
//! and the process path. No device IO — `trib-audio` drives it; tests
//! drive it directly.

mod compiled;
mod engine;
mod playback;
mod record;
mod rings;
mod slots;

pub use compiled::{CompileOutput, CompiledGraph, ParamMap, compile};
pub use engine::{EngineHandle, GraphEngine, engine_pair};
pub use playback::{PlaybackSet, PlaybackShared, PlaybackTrack, lane_audible};
pub use record::{RECORD_RING_SECS, RecordSet, RecordTrack};
pub use rings::{
    CMD_RING_CAPACITY, EngineCommand, EqIx, FlagIx, MAX_METERS, METER_RING_CAPACITY,
    MONITOR_RING_CAPACITY, MeterBlock, MonitorTarget, ParamIx, RETIRE_RING_CAPACITY, Retired,
};
pub use slots::{InputSlots, MAX_INPUT_CHANNELS, SlotError};
