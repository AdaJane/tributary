use trib_engine::GraphEngine;

#[derive(Debug, Clone, Copy)]
pub struct StreamConfig {
    pub sample_rate: u32,
    pub block_size: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no usable device: {0}")]
    Device(String),
    #[error("stream failed: {0}")]
    Stream(String),
}

/// A request to start feeding one device into the engine frame.
#[derive(Debug, Clone)]
pub struct OpenInput {
    /// `None` = the system default input.
    pub device: Option<String>,
    /// Channels to open — decided by the caller from a fresh enumeration.
    pub channels: u16,
    /// Where the device's channels land in the fixed engine frame.
    pub offset: u16,
    /// Route through the Pulse capture path (see `InputDeviceInfo.pulse`).
    pub pulse: bool,
}

/// Control-side snapshot of one open input stream.
#[derive(Debug, Clone)]
pub struct InputStreamStatus {
    pub device: Option<String>,
    pub offset: u16,
    pub channels: u16,
    /// The stream's error callback fired since open.
    pub failed: bool,
    pub underruns: u64,
}

/// A running audio stream. Dropping it stops the audio thread. The input
/// methods are control-side and may block — never call from the audio
/// thread.
pub trait StreamHandle: Send {
    fn open_input(&self, req: OpenInput) -> Result<(), AudioError>;
    fn close_input(&self, device: Option<&str>) -> Result<(), AudioError>;
    fn input_status(&self) -> Vec<InputStreamStatus>;
}

/// One input device as the OS names it. The ACTIVE device is the system
/// default — what unqualified patches follow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDeviceInfo {
    /// Stable identity — what patches store and opens use.
    pub name: String,
    /// The friendly print the system sound menu shows, when the source
    /// layer provides one (Pulse/PipeWire descriptions).
    pub description: Option<String>,
    pub channels: u16,
    pub active: bool,
    /// Open this via the Pulse capture path (by-name source targeting)
    /// rather than a raw ALSA device.
    pub pulse: bool,
}

/// The backend seam. `start` consumes the engine: the audio thread owns it
/// for the stream's lifetime.
pub trait AudioBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Enumerate input devices as the OS reports them, freshly on every
    /// call (hotplug shows up on the next look). Control-side only — may
    /// block; never call from the audio thread.
    fn input_devices(&self) -> Vec<InputDeviceInfo>;
    fn start(
        &self,
        config: &StreamConfig,
        engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError>;
}
