//! The real-time engine: lock-free control↔audio protocol, graph compiler,
//! and the process path. No device IO — `trib-audio` drives it; tests
//! drive it directly.

mod compiled;
mod engine;
mod instrument;
mod playback;
mod record;
mod rings;
mod slots;

pub use compiled::{CompileOutput, CompiledGraph, ParamMap, compile};
pub use engine::{EngineHandle, GraphEngine, engine_pair};
pub use instrument::{
    INSTRUMENT_CHANNELS, Instrument, InstrumentRack, KeyFilter, MAX_INSTRUMENT_CHANNELS,
    MIDI_RING_CAPACITY, MidiBinding, MidiEvent, SYNTH_SUB_BLOCK, VoiceSpec,
};
/// Re-exported so the daemon loads soundfonts against exactly the version
/// the engine renders with. Two copies of `rustysynth` in one binary would
/// type-check and then fail to hand a `SoundFont` across.
pub use rustysynth::{SoundFont, SoundFontError};
/// A generated, genuinely loadable SoundFont for tests that need one.
/// Public so the daemon's tests can exercise loading without a second copy
/// of the generator — and without committing a licence-encumbered binary.
pub mod test_support {
    pub use crate::instrument::fixture::sine_soundfont;
}
pub use playback::{PlaybackSet, PlaybackShared, PlaybackTrack, lane_audible};
pub use record::{MIDI_CAPTURE_CAPACITY, MidiCapture, RECORD_RING_SECS, RecordSet, RecordTrack};
pub use rings::{
    CMD_RING_CAPACITY, EngineCommand, EqIx, FlagIx, MAX_METERS, METER_RING_CAPACITY,
    MONITOR_RING_CAPACITY, MeterBlock, MonitorTarget, ParamIx, RETIRE_RING_CAPACITY, Retired,
};
pub use slots::{InputSlots, MAX_INPUT_CHANNELS, SlotError};
