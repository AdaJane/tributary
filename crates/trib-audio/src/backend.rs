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
    /// The source's own channel map, when known — passed to the capture so
    /// the stream is opened in the SOURCE's positions rather than a
    /// synthesised default. Load-bearing, not decorative: see
    /// `pulse::parec_args`.
    pub channel_map: Option<String>,
}

/// Control-side snapshot of one open input stream.
#[derive(Debug, Clone)]
pub struct InputStreamStatus {
    pub device: Option<String>,
    pub offset: u16,
    pub channels: u16,
    /// The stream's error callback fired since open.
    pub failed: bool,
    /// Frames the engine failed to take in time (the ring ran dry).
    pub underruns: u64,
    /// Whole frames the device produced that the ring had no room for.
    pub overruns: u64,
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
    /// What the device exposes RIGHT NOW. On a Pulse/PipeWire system this
    /// is a property of the card's active profile, not of the hardware —
    /// see [`CardInfo`].
    pub channels: u16,
    pub active: bool,
    /// Open this via the Pulse capture path (by-name source targeting)
    /// rather than a raw ALSA device.
    pub pulse: bool,
    /// The card this device belongs to, when the platform has cards.
    /// Profiles are selected on the card, never on the device.
    pub card: Option<String>,
    /// The device's own channel map ("aux0,aux1,…"), when known. Fed
    /// straight into the capture: the Pulse layer routes by position name,
    /// so opening in the source's own positions is what makes stream
    /// channel `i` carry source channel `i`.
    pub channel_map: Option<String>,
    /// The source layer's mute flag. A muted device opens, streams and
    /// delivers silence with no error anywhere, so it has to be reported
    /// or the console cannot explain a dead meter.
    pub muted: bool,
    /// The quietest channel's volume as a percentage of unity, when the
    /// source layer says. None = unknown, which is not the same as 0.
    pub volume_percent: Option<u32>,
}

/// A sound card and the profiles it can be switched between.
///
/// The active profile decides how many channels the card's devices expose,
/// so a multichannel interface can be present, healthy, and still show only
/// a couple of inputs until it is switched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardInfo {
    pub name: String,
    pub active_profile: String,
    /// Capture-capable profiles only, in the platform's own order.
    pub profiles: Vec<CardProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardProfile {
    pub name: String,
    pub description: String,
}

/// The backend seam. `start` consumes the engine: the audio thread owns it
/// for the stream's lifetime.
pub trait AudioBackend: Send + Sync {
    fn name(&self) -> &'static str;
    /// Enumerate input devices as the OS reports them, freshly on every
    /// call (hotplug shows up on the next look). Control-side only — may
    /// block; never call from the audio thread.
    fn input_devices(&self) -> Vec<InputDeviceInfo>;
    /// Sound cards and their selectable profiles, freshly on every call.
    /// Empty where the platform has no such concept (raw ALSA, the fake
    /// backend) — the caller then simply has nothing to offer.
    fn input_cards(&self) -> Vec<CardInfo> {
        Vec::new()
    }
    /// Put a card into a profile. Devices change name, width and map, so
    /// the caller must re-enumerate rather than trust an earlier read.
    fn set_card_profile(&self, _card: &str, _profile: &str) -> Result<(), AudioError> {
        Err(AudioError::Device(
            "this audio backend has no card profiles".into(),
        ))
    }
    fn start(
        &self,
        config: &StreamConfig,
        engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError>;
}
