//! Persistence: the project directory, its TOML manifest, and the take
//! writer. Textual manifest + append-only WAVs — no database.

mod flac;
mod format;
mod midi;
mod peaks;
mod project;
mod reader;
mod writer;

pub use format::RecordFormat;
pub use midi::{MidiSink, MidiTrackReport, TICKS_PER_SECOND, build_smf, tick_of};
pub use peaks::{
    PEAK_SAMPLES_PER_BIN, PeakAccum, compute_from_audio, read_or_compute, read_sidecar,
};
pub use project::{
    Project, ProjectError, ProjectManifest, SCHEMA_VERSION, SessionDir, SessionSummary, TakeInfo,
    TakeMidiTrackInfo, TakeTrackInfo, create_project, delete_session, delete_take, list_sessions,
    list_takes, load_latest, manifest_of, open_session, rename_session, resolve, save_manifest,
    validate_name,
};
pub use reader::{FeederHandle, PLAYBACK_RING_SECS, PlaybackSource, TrackFile, open_and_prime};
pub use writer::{PeakBatch, PeaksTap, TakeTrackSpec, TrackSink, file_slug, spawn_writer};
