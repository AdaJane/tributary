//! Device seam: the one dyn-trait boundary in the workspace. The daemon
//! picks a backend at boot; everything upstream sees only `AudioBackend`.

mod assembler;
mod backend;
#[cfg(feature = "cpal-backend")]
mod cpal_backend;
mod fake;
#[cfg(feature = "cpal-backend")]
mod pulse;
#[cfg(feature = "cpal-backend")]
mod stall;

pub use backend::{
    AudioBackend, AudioError, InputDeviceInfo, InputStreamStatus, OpenInput, StreamConfig,
    StreamHandle,
};
#[cfg(feature = "cpal-backend")]
pub use cpal_backend::CpalBackend;
pub use fake::FakeBackend;
