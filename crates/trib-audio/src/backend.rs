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

/// A request to start draining part of the engine's output plane to a
/// device. The mirror of [`OpenInput`].
#[derive(Debug, Clone)]
pub struct OpenOutput {
    /// `None` = the system default output.
    pub device: Option<String>,
    /// Channels to open — decided by the caller from a fresh enumeration.
    pub channels: u16,
    /// Where this device's channels come FROM in the fixed engine plane.
    pub offset: u16,
    /// Route through the Pulse/PipeWire sink path rather than raw ALSA.
    pub pulse: bool,
    /// The sink's own channel map, when known. Load-bearing for exactly
    /// the reason it is on the input side — see `pulse::parec_args`.
    pub channel_map: Option<String>,
}

/// Control-side snapshot of one open output stream.
#[derive(Debug, Clone)]
pub struct OutputStreamStatus {
    pub device: Option<String>,
    pub offset: u16,
    pub channels: u16,
    /// The stream's error callback fired since open.
    pub failed: bool,
    /// Frames the device asked for that the engine had not produced.
    pub underruns: u64,
    /// Whole frames the engine produced that the ring had no room for.
    pub overruns: u64,
    /// ALSA xruns recovered on this stream. Always 0 off the real-time
    /// backend, which is the only one that can see them.
    pub xruns: u64,
    /// Worst `engine.process` block observed this second, in microseconds.
    ///
    /// The number that decides whether a period size is achievable, and
    /// measured rather than estimated — a latency figure nobody measured
    /// is a wish.
    pub worst_block_us: u32,
}

/// How a thread is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduling {
    /// Real-time, first-in-first-out.
    Fifo,
    /// Ordinary time-sharing. Audio still runs; xruns under load are
    /// expected and are not a bug in the daemon.
    Other,
    /// This backend has no audio thread to schedule.
    NotApplicable,
}

/// What the machine actually granted the audio thread.
///
/// Probed once — none of it can change while the daemon runs — and
/// reported rather than assumed, because a console that claims real-time
/// scheduling it did not get will be blamed for xruns nobody can explain.
#[derive(Debug, Clone)]
pub struct RealtimeStatus {
    pub scheduling: Scheduling,
    pub priority: Option<u32>,
    pub memory_locked: bool,
    /// Why it is not better than this, in a sentence someone can act on.
    pub reason: Option<String>,
}

impl RealtimeStatus {
    pub fn not_applicable() -> Self {
        RealtimeStatus {
            scheduling: Scheduling::NotApplicable,
            priority: None,
            memory_locked: false,
            reason: None,
        }
    }
}

/// A running audio stream. Dropping it stops the audio thread. The input
/// and output methods are control-side and may block — never call from the
/// audio thread.
pub trait StreamHandle: Send {
    fn open_input(&self, req: OpenInput) -> Result<(), AudioError>;
    fn close_input(&self, device: Option<&str>) -> Result<(), AudioError>;
    fn input_status(&self) -> Vec<InputStreamStatus>;

    /// Start draining a slice of the engine plane to a device. Defaulted
    /// so a capture-only backend refuses with a sentence rather than
    /// silently accepting a patch that will never be heard.
    fn open_output(&self, _req: OpenOutput) -> Result<(), AudioError> {
        Err(AudioError::Device(
            "this audio backend has no patchable outputs".into(),
        ))
    }

    /// Closing something that was never open is not an error — the
    /// orchestrator reconciles toward a wanted set and must be able to
    /// converge from any starting point.
    fn close_output(&self, _device: Option<&str>) -> Result<(), AudioError> {
        Ok(())
    }

    fn output_status(&self) -> Vec<OutputStreamStatus> {
        Vec::new()
    }
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

/// One output device as the OS names it.
///
/// The mirror of [`InputDeviceInfo`], field for field, because the
/// console's output board is the input board reflected and a field missing
/// on one side is a jack the user cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDeviceInfo {
    /// Stable identity — what patches store and opens use.
    pub name: String,
    /// The friendly print the system sound menu shows, when the sink layer
    /// provides one.
    pub description: Option<String>,
    /// What the device exposes RIGHT NOW. On a Pulse/PipeWire system this
    /// is a property of the card's active profile, not of the hardware.
    pub channels: u16,
    /// The system default output — where the monitor pair goes unless
    /// something else is said.
    pub active: bool,
    pub pulse: bool,
    pub card: Option<String>,
    pub channel_map: Option<String>,
    /// The sink layer's mute flag. A muted output opens, streams and puts
    /// nothing in the room, and unlike a muted INPUT — whose dead meter at
    /// least shows the silence — an output has no meter at all. This field
    /// and `volume_percent` are the only place a dead PA is explainable.
    pub muted: bool,
    pub volume_percent: Option<u32>,
    /// Channels this device reserves for the monitor pair and will not
    /// offer as patch destinations.
    ///
    /// Named rather than quietly subtracted from `channels`, because an
    /// output the console draws as missing is exactly the failure the
    /// input side already learned to avoid.
    pub monitor_channels: u16,
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
    /// Whether this backend can drive patchable outputs at all.
    ///
    /// False by default: a backend that cannot answers **501** rather than
    /// pretending a patch will ever be heard. The monitor pair is not
    /// covered by this — every backend that makes sound has one.
    fn supports_outputs(&self) -> bool {
        false
    }

    /// Output devices, freshly on every call. Empty where the backend has
    /// nothing to offer beyond its own monitor — honest, not a failure:
    /// the console then simply has nothing to patch to.
    fn output_devices(&self) -> Vec<OutputDeviceInfo> {
        Vec::new()
    }

    /// How the machine's real-time posture actually came out.
    fn realtime_status(&self) -> RealtimeStatus {
        RealtimeStatus::not_applicable()
    }

    fn start(
        &self,
        config: &StreamConfig,
        engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError>;
}
