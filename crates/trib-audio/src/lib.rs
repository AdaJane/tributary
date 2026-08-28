//! Device seam: the one dyn-trait boundary in the workspace. The daemon
//! picks a backend at boot; everything upstream sees only `AudioBackend`.

#[cfg(feature = "alsa-backend")]
mod alsa_backend;
mod assembler;
mod backend;
#[cfg(feature = "cpal-backend")]
mod cpal_backend;
#[cfg(feature = "cpal-backend")]
mod disassembler;
mod fake;
mod probe;
#[cfg(feature = "cpal-backend")]
mod pulse;
mod realtime;
#[cfg(feature = "cpal-backend")]
mod stall;

#[cfg(feature = "alsa-backend")]
pub use alsa_backend::AlsaBackend;
pub use backend::{
    AudioBackend, AudioError, BackendStatus, CardInfo, CardProfile, InputDeviceInfo,
    InputStreamStatus, OpenInput, OpenOutput, OutputDeviceInfo, OutputStreamStatus, RealtimeStatus,
    Scheduling, StreamConfig, StreamHandle,
};
#[cfg(feature = "cpal-backend")]
pub use cpal_backend::CpalBackend;
pub use fake::FakeBackend;
pub use realtime::{MEMLOCK_FLOOR, RT_PRIORITY};
