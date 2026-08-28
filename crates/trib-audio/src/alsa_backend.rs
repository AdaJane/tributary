//! The real-time backend: one ALSA `hw:` card owned outright.
//!
//! Capture and playback are opened together and `snd_pcm_link`ed, so the
//! two directions share ONE hardware clock. That single fact removes, by
//! construction: the `parec` child-process hop, the server's resampling,
//! the whole `--channel-map` remapping class of bug (see
//! [`crate::pulse::parec_args`]), and the drift underruns that two
//! independent clocks produce.
//!
//! The engine runs IN this thread, clocked by the card. That reverses the
//! decoupling the cpal backend needed, and the reversal is argued rather
//! than assumed: the failure that forced it was a *server* silently
//! ceasing to call back, which is why `StallWatch` had to infer a stall
//! from missing beats. A `hw:` device cannot fail silently — every stall
//! surfaces as `-EPIPE`, `-ESTRPIPE` or `-ENODEV` from `readi`/`writei`.
//! And a timer thread here would be a SECOND clock, reintroducing exactly
//! the drift this backend exists to remove.
//!
//! The old lesson is kept in full by the two-mode clock: any fatal device
//! error drops to a timer within one block, so meters and recording never
//! stop, and the card is retried until it comes back.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use alsa::pcm::{Access, Format, HwParams, PCM, State};
use alsa::{Direction, ValueOr};
// Named errno constants rather than the raw integers `alsa::Error::errno`
// hands back: 32 and 86 are the kind of magic number that survives a
// rewrite and stops meaning anything.
use rustix::io::Errno;
use trib_engine::{GraphEngine, MAX_INPUT_CHANNELS, MAX_OUTPUT_CHANNELS, MONITOR_CHANNELS};

use crate::backend::{
    AudioBackend, AudioError, BackendStatus, InputDeviceInfo, InputStreamStatus, OpenInput,
    OpenOutput, OutputDeviceInfo, OutputStreamStatus, RealtimeStatus, StreamConfig, StreamHandle,
};
use crate::probe::choose_card;
use crate::realtime;

/// Periods in the ring. Three is the smallest that tolerates one late
/// wake-up without an xrun; two makes every scheduling hiccup audible.
const PERIODS: u32 = 3;

/// How long a dead card waits before the next reopen attempt. Matches the
/// cpal backend's rebuild window — the same lesson, the same cadence.
const REOPEN_AFTER: Duration = Duration::from_secs(5);

/// Full scale for a 32-bit ALSA frame: 2^31, not `i32::MAX`.
///
/// Deliberately the power of two. `i32::MAX` is not representable in f32 —
/// it rounds to 2^31 anyway — so writing it would only disguise what the
/// arithmetic actually does, and would make the round-trip below wrong by
/// a bit at every amplitude.
///
/// A card carrying 24 significant bits in a 32-bit slot still scales by
/// the whole range; the low byte is simply zero.
const S32_SCALE: f32 = 2_147_483_648.0;
const S16_SCALE: f32 = 32_768.0;

/// The sample format negotiated with the card.
///
/// Not a detail that can be assumed: no device on this development machine
/// offers a float format at all — `hw:0,0` takes S16/S24/S32_LE and a USB
/// dock takes S16_LE only. The engine is f32 and the plug layer is off, so
/// something has to convert, and it has to know what it is converting to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    S32,
    S16,
}

/// The clamp is load-bearing, not decorative: it is what turns a hot mix
/// into clipping rather than the loudest sound a PA can make.
///
/// Rust's float-to-int `as` saturates rather than wrapping, so the cast is
/// a second net under the clamp — but only the clamp keeps a +2.0 sample
/// at full scale instead of pinned to a rail, and only the clamp is
/// visible to a reader wondering whether this is safe.
#[inline]
pub fn to_s32(x: f32) -> i32 {
    (x.clamp(-1.0, 1.0) * S32_SCALE) as i32
}

#[inline]
pub fn to_s16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * S16_SCALE) as i16
}

#[inline]
pub fn from_s32(x: i32) -> f32 {
    x as f32 / S32_SCALE
}

#[inline]
pub fn from_s16(x: i16) -> f32 {
    x as f32 / S16_SCALE
}

/// Telemetry the control side reads while the RT thread runs.
#[derive(Debug, Default)]
struct RtShared {
    xruns: AtomicU64,
    underruns: AtomicU64,
    worst_block_us: AtomicU32,
    /// The card died and the timer clock is carrying metering.
    degraded: AtomicBool,
    stop: AtomicBool,
}

/// One `hw:` card owned outright: capture and playback linked on one clock.
struct Duplex {
    capture: PCM,
    playback: PCM,
    period: usize,
    in_channels: u16,
    out_channels: u16,
    format: SampleFormat,
}

pub struct AlsaBackend {
    /// The card to take, e.g. `hw:1`. `None` = the first card that opens
    /// duplex at the engine rate, in ALSA index order.
    device: Option<String>,
    /// What the audio thread actually holds, written by that thread on
    /// every open and death, so enumeration reports what was negotiated
    /// rather than what was asked for — and reports NOTHING while the
    /// card is down.
    health: Arc<Mutex<CardHealth>>,
    status: Arc<Mutex<RealtimeStatus>>,
    shared: Arc<RtShared>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenedCard {
    name: String,
    in_channels: u16,
    out_channels: u16,
}

/// The card's state as one value, so "open" and "why not" can never
/// disagree: coming up clears the error, going down clears the card.
///
/// `generation` moves on every transition. The orchestrator reads it to
/// learn that the device set changed without enumerating on a timer.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CardHealth {
    opened: Option<OpenedCard>,
    last_error: Option<String>,
    generation: u64,
}

impl CardHealth {
    fn came_up(&mut self, card: OpenedCard) {
        self.opened = Some(card);
        self.last_error = None;
        self.generation += 1;
    }

    fn went_down(&mut self, why: String) {
        self.opened = None;
        self.last_error = Some(why);
        self.generation += 1;
    }
}

impl AlsaBackend {
    pub fn new(device: Option<String>) -> Self {
        AlsaBackend {
            device,
            health: Arc::new(Mutex::new(CardHealth::default())),
            status: Arc::new(Mutex::new(RealtimeStatus::not_applicable())),
            shared: Arc::new(RtShared::default()),
        }
    }

    fn opened(&self) -> Option<OpenedCard> {
        self.health.lock().expect("card health lock").opened.clone()
    }
}

struct AlsaStream {
    shared: Arc<RtShared>,
    health: Arc<Mutex<CardHealth>>,
    /// Which slice of the engine plane the card's playback takes, and
    /// which slice of the input frame its capture fills. Read by the RT
    /// thread every block; written by the orchestrator.
    routing: Arc<Mutex<Routing>>,
    thread: Option<JoinHandle<()>>,
}

/// Where the one card sits in the engine's frames.
#[derive(Debug, Default, Clone, Copy)]
struct Routing {
    input_offset: u16,
    input_open: bool,
    output_offset: u16,
    output_open: bool,
}

impl StreamHandle for AlsaStream {
    fn open_input(&self, req: OpenInput) -> Result<(), AudioError> {
        let card = self.health.lock().expect("card health lock").opened.clone();
        let Some(card) = card else {
            return Err(AudioError::Device("the card is not open".into()));
        };
        if req.device.as_deref().is_some_and(|name| name != card.name) {
            return Err(second_card_error(&card.name));
        }
        let mut routing = self.routing.lock().expect("routing lock");
        routing.input_offset = req.offset;
        routing.input_open = true;
        Ok(())
    }

    fn close_input(&self, _device: Option<&str>) -> Result<(), AudioError> {
        self.routing.lock().expect("routing lock").input_open = false;
        Ok(())
    }

    fn input_status(&self) -> Vec<InputStreamStatus> {
        let card = self.health.lock().expect("card health lock").opened.clone();
        let routing = *self.routing.lock().expect("routing lock");
        card.filter(|_| routing.input_open)
            .map(|card| InputStreamStatus {
                device: Some(card.name),
                offset: routing.input_offset,
                channels: card.in_channels,
                failed: self.shared.degraded.load(Ordering::Relaxed),
                underruns: self.shared.underruns.load(Ordering::Relaxed),
                overruns: 0,
            })
            .into_iter()
            .collect()
    }

    fn open_output(&self, req: OpenOutput) -> Result<(), AudioError> {
        let card = self.health.lock().expect("card health lock").opened.clone();
        let Some(card) = card else {
            return Err(AudioError::Device("the card is not open".into()));
        };
        if req.device.as_deref().is_some_and(|name| name != card.name) {
            return Err(second_card_error(&card.name));
        }
        let mut routing = self.routing.lock().expect("routing lock");
        routing.output_offset = req.offset;
        routing.output_open = true;
        Ok(())
    }

    fn close_output(&self, _device: Option<&str>) -> Result<(), AudioError> {
        self.routing.lock().expect("routing lock").output_open = false;
        Ok(())
    }

    fn output_status(&self) -> Vec<OutputStreamStatus> {
        let card = self.health.lock().expect("card health lock").opened.clone();
        let routing = *self.routing.lock().expect("routing lock");
        card.filter(|_| routing.output_open)
            .map(|card| OutputStreamStatus {
                device: Some(card.name),
                offset: routing.output_offset,
                channels: card.out_channels.saturating_sub(MONITOR_CHANNELS),
                failed: self.shared.degraded.load(Ordering::Relaxed),
                underruns: self.shared.underruns.load(Ordering::Relaxed),
                overruns: 0,
                xruns: self.shared.xruns.load(Ordering::Relaxed),
                worst_block_us: self.shared.worst_block_us.load(Ordering::Relaxed),
            })
            .into_iter()
            .collect()
    }
}

impl Drop for AlsaStream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The refusal a second interface gets, and why.
fn second_card_error(card: &str) -> AudioError {
    AudioError::Device(format!(
        "the real-time backend owns {card} alone — a second interface runs on its own \
         clock, and two clocks is the drift this backend exists to remove"
    ))
}

impl AudioBackend for AlsaBackend {
    fn name(&self) -> &'static str {
        "alsa"
    }

    fn input_devices(&self) -> Vec<InputDeviceInfo> {
        self.opened()
            .iter()
            .map(|card| InputDeviceInfo {
                name: card.name.clone(),
                description: None,
                channels: card.in_channels,
                active: true,
                pulse: false,
                // `hw:` has no card-profile concept: the width is the
                // hardware's, not a choice, which is precisely what this
                // backend is for.
                card: None,
                channel_map: None,
                muted: false,
                volume_percent: None,
            })
            .collect()
    }

    fn supports_outputs(&self) -> bool {
        true
    }

    fn output_devices(&self) -> Vec<OutputDeviceInfo> {
        self.opened()
            .iter()
            .map(|card| OutputDeviceInfo {
                name: card.name.clone(),
                description: None,
                channels: card.out_channels,
                active: true,
                pulse: false,
                card: None,
                channel_map: None,
                muted: false,
                volume_percent: None,
                // The control-room feed shares this card: its first two
                // channels are the monitor, so the patch bay starts at the
                // card's physical output 3.
                monitor_channels: MONITOR_CHANNELS,
            })
            .collect()
    }

    fn realtime_status(&self) -> RealtimeStatus {
        self.status.lock().expect("rt status lock").clone()
    }

    fn status(&self) -> BackendStatus {
        let health = self.health.lock().expect("card health lock").clone();
        BackendStatus {
            running: health.opened.is_some(),
            card: health.opened.map(|card| card.name),
            error: health.last_error,
            realtime: self.realtime_status(),
        }
    }

    fn generation(&self) -> u64 {
        self.health.lock().expect("card health lock").generation
    }

    fn start(
        &self,
        config: &StreamConfig,
        engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError> {
        let config = *config;
        let wanted = self.device.clone();
        // Opened here, on the caller's thread, so the boot line names the
        // card while the daemon is still saying what it found. A card that
        // will not open is NOT a boot error: the thread starts on the
        // timer clock and retries, because on an appliance the interface
        // may be plugged in after boot, or PipeWire may have been probing
        // it at the exact moment we asked. Silence-until-restart was the
        // failure this replaces; the console reports the state meanwhile.
        let duplex = match open_card(wanted.as_deref(), &config) {
            Ok((name, duplex)) => {
                self.health
                    .lock()
                    .expect("card health lock")
                    .came_up(opened_card(&name, &duplex));
                log_card_open(&name, &config, &duplex);
                Some(duplex)
            }
            Err(e) => {
                tracing::error!(
                    %e,
                    "real-time card failed to open; metering and recording run on the timer \
                     clock, retrying every {} s",
                    REOPEN_AFTER.as_secs()
                );
                self.health
                    .lock()
                    .expect("card health lock")
                    .went_down(e.to_string());
                self.shared.degraded.store(true, Ordering::Relaxed);
                None
            }
        };

        let routing = Arc::new(Mutex::new(Routing::default()));
        let shared = self.shared.clone();
        let status = self.status.clone();
        let health = self.health.clone();
        let thread_routing = routing.clone();
        let thread = std::thread::Builder::new()
            .name("trib-alsa-audio".into())
            .spawn(move || {
                let granted = realtime::apply(realtime::posture(realtime::limits()));
                match &granted.reason {
                    Some(reason) => tracing::warn!(%reason, "real-time audio, degraded"),
                    None => tracing::info!(
                        priority = granted.priority,
                        "real-time audio: SCHED_FIFO, memory locked"
                    ),
                }
                *status.lock().expect("rt status lock") = granted;
                run(
                    config,
                    engine,
                    duplex,
                    wanted.as_deref(),
                    thread_routing,
                    shared,
                    health,
                );
            })
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        Ok(Box::new(AlsaStream {
            shared: self.shared.clone(),
            health: self.health.clone(),
            routing,
            thread: Some(thread),
        }))
    }
}

fn opened_card(name: &str, duplex: &Duplex) -> OpenedCard {
    OpenedCard {
        name: name.to_owned(),
        in_channels: duplex.in_channels,
        out_channels: duplex.out_channels,
    }
}

fn log_card_open(name: &str, config: &StreamConfig, duplex: &Duplex) {
    tracing::info!(
        card = %name,
        model = card_model(name).as_deref().unwrap_or("?"),
        rate = config.sample_rate,
        period = duplex.period,
        format = ?duplex.format,
        inputs = duplex.in_channels,
        outputs = duplex.out_channels,
        "real-time card open, capture and playback linked on one clock"
    );
}

/// The kernel's name for a `hw:N` card ("UMC1820"), for the journal —
/// `hw:0` alone says nothing about whether the right interface was taken.
fn card_model(name: &str) -> Option<String> {
    let index = name.strip_prefix("hw:")?.split(',').next()?.parse().ok()?;
    alsa::Card::new(index).get_name().ok()
}

/// Every card the kernel has, as `hw:N`, in index order.
fn alsa_cards() -> Vec<String> {
    alsa::card::Iter::new()
        .filter_map(Result::ok)
        .map(|card| format!("hw:{}", card.get_index()))
        .collect()
}

/// Open the configured card, or — with none configured — the first card
/// that opens duplex at the engine rate.
fn open_card(wanted: Option<&str>, config: &StreamConfig) -> Result<(String, Duplex), AudioError> {
    match wanted {
        Some(name) => open_duplex(name, config).map(|duplex| (name.to_owned(), duplex)),
        None => {
            choose_card(alsa_cards(), |name| open_duplex(name, config)).map_err(AudioError::Device)
        }
    }
}

/// Configure one direction of the card.
fn configure(
    pcm: &PCM,
    config: &StreamConfig,
    format: SampleFormat,
) -> Result<(u16, usize), AudioError> {
    let hwp = HwParams::any(pcm).map_err(|e| AudioError::Device(format!("hw params: {e}")))?;
    let fail = |what: &str, e: alsa::Error| AudioError::Device(format!("{what}: {e}"));
    hwp.set_access(Access::RWInterleaved)
        .map_err(|e| fail("access", e))?;
    // Explicit: a name that resolves to a plug chain must be REFUSED
    // rather than silently converting behind us, which is the whole point
    // of taking a `hw:` device.
    hwp.set_rate_resample(false)
        .map_err(|e| fail("resample", e))?;
    hwp.set_format(match format {
        SampleFormat::S32 => Format::s32(),
        SampleFormat::S16 => Format::s16(),
    })
    .map_err(|e| fail("format", e))?;
    hwp.set_rate(config.sample_rate, ValueOr::Nearest)
        .map_err(|e| fail("rate", e))?;
    hwp.set_period_size_near(config.block_size as i64, ValueOr::Nearest)
        .map_err(|e| fail("period", e))?;
    hwp.set_periods(PERIODS, ValueOr::Nearest)
        .map_err(|e| fail("periods", e))?;
    let channels = hwp
        .get_channels_max()
        .map_err(|e| fail("channels", e))?
        .min(u32::try_from(MAX_INPUT_CHANNELS).unwrap_or(u32::MAX));
    hwp.set_channels(channels)
        .map_err(|e| fail("set channels", e))?;
    pcm.hw_params(&hwp).map_err(|e| fail("apply", e))?;

    // Read the rate BACK and refuse a mismatch. A card that lands on
    // 44100 when asked for 48000 makes every timing in the daemon a lie —
    // the take's duration, the playhead, the sidecar's tick map.
    let landed = hwp.get_rate().map_err(|e| fail("rate readback", e))?;
    if landed != config.sample_rate {
        return Err(AudioError::Device(format!(
            "the card runs at {landed} Hz and the engine at {}; it cannot be resampled \
             without putting a converter back in the path this backend exists to remove",
            config.sample_rate
        )));
    }
    let period = hwp
        .get_period_size()
        .map_err(|e| fail("period readback", e))? as usize;
    let channels = hwp
        .get_channels()
        .map_err(|e| fail("channel readback", e))? as u16;
    Ok((channels, period))
}

/// Open one card duplex and link the two directions onto one clock.
fn open_duplex(name: &str, config: &StreamConfig) -> Result<Duplex, AudioError> {
    let open = |dir: Direction| {
        PCM::new(name, dir, false).map_err(|e| AudioError::Device(format!("{name} ({dir:?}): {e}")))
    };
    let capture = open(Direction::Capture)?;
    let playback = open(Direction::Playback)?;

    // S32 first, S16 as the fallback. Nothing here offers float, so
    // something must convert either way; the only question is the width.
    let mut chosen = None;
    let mut last = None;
    for format in [SampleFormat::S32, SampleFormat::S16] {
        match (
            configure(&capture, config, format),
            configure(&playback, config, format),
        ) {
            (Ok(cap), Ok(play)) => {
                chosen = Some((format, cap, play));
                break;
            }
            (Err(e), _) | (_, Err(e)) => last = Some(e),
        }
    }
    let Some((format, (in_channels, period), (out_channels, _))) = chosen else {
        return Err(last.unwrap_or_else(|| AudioError::Device("no usable format".into())));
    };

    {
        let swp = playback
            .sw_params_current()
            .map_err(|e| AudioError::Device(format!("sw params: {e}")))?;
        swp.set_start_threshold((period * PERIODS as usize) as i64)
            .map_err(|e| AudioError::Device(format!("start threshold: {e}")))?;
        playback
            .sw_params(&swp)
            .map_err(|e| AudioError::Device(format!("sw params apply: {e}")))?;

        // The link is the whole point: after this the two directions share
        // one clock, so a frame captured and a frame played are the same
        // frame. Scoped so the borrows end before the PCMs move into the
        // struct below.
        capture
            .link(&playback)
            .map_err(|e| AudioError::Device(format!("link: {e}")))?;
    }

    Ok(Duplex {
        capture,
        playback,
        period,
        in_channels,
        out_channels,
        format,
    })
}

/// Why the device loop stopped.
enum Exit {
    /// Asked to stop.
    Shutdown,
    /// The card is unusable; fall back to the timer clock and retry.
    Dead(String),
}

#[allow(clippy::too_many_arguments)]
fn run(
    config: StreamConfig,
    mut engine: GraphEngine,
    mut duplex: Option<Duplex>,
    wanted: Option<&str>,
    routing: Arc<Mutex<Routing>>,
    shared: Arc<RtShared>,
    health: Arc<Mutex<CardHealth>>,
) {
    let mut input = vec![0.0f32; config.block_size * MAX_INPUT_CHANNELS];
    let mut output = trib_engine::output_buffer(config.block_size);
    let mut scratch_in = vec![0i32; config.block_size * MAX_INPUT_CHANNELS];
    let mut scratch_out = vec![0i32; config.block_size * MAX_INPUT_CHANNELS];
    let mut scratch_in16 = vec![0i16; config.block_size * MAX_INPUT_CHANNELS];
    let mut scratch_out16 = vec![0i16; config.block_size * MAX_INPUT_CHANNELS];

    while !shared.stop.load(Ordering::Relaxed) {
        match duplex.take() {
            Some(card) => {
                shared.degraded.store(false, Ordering::Relaxed);
                let exit = run_device(
                    &card,
                    &mut engine,
                    &routing,
                    &shared,
                    &mut input,
                    &mut output,
                    (&mut scratch_in, &mut scratch_out),
                    (&mut scratch_in16, &mut scratch_out16),
                );
                match exit {
                    Exit::Shutdown => break,
                    Exit::Dead(why) => {
                        tracing::error!(
                            %why,
                            "the real-time card died — metering and recording continue on \
                             the timer clock; retrying the card"
                        );
                        shared.degraded.store(true, Ordering::Relaxed);
                        health.lock().expect("card health lock").went_down(why);
                    }
                }
            }
            None => {
                // The timer arm: the promise the cpal backend made and this
                // one keeps. An output that genuinely dies costs monitor
                // audio and NOTHING else — meters and takes keep running.
                let deadline = Instant::now() + REOPEN_AFTER;
                let block =
                    Duration::from_secs_f64(config.block_size as f64 / config.sample_rate as f64);
                let mut next = Instant::now() + block;
                while Instant::now() < deadline && !shared.stop.load(Ordering::Relaxed) {
                    input.fill(0.0);
                    engine.process(&input, MAX_INPUT_CHANNELS, &mut output);
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else if now - next > block * 8 {
                        next = now;
                    }
                    next += block;
                }
                if shared.stop.load(Ordering::Relaxed) {
                    break;
                }
                match open_card(wanted, &config) {
                    Ok((name, card)) => {
                        let opened = opened_card(&name, &card);
                        let mut health = health.lock().expect("card health lock");
                        // A card of a different width must not be fed
                        // through the slots the orchestrator sized for
                        // the old one: drop the routing and let it re-open
                        // at the new width (it watches `generation`).
                        if let Some(before) = &health.opened
                            && (before.in_channels, before.out_channels)
                                != (opened.in_channels, opened.out_channels)
                        {
                            tracing::warn!(
                                card = %name,
                                "the card came back a different width; patches re-open at \
                                 the new width"
                            );
                            *routing.lock().expect("routing lock") = Routing::default();
                        }
                        log_card_open(&name, &config, &card);
                        tracing::info!(card = %name, "the real-time card came back");
                        health.came_up(opened);
                        duplex = Some(card);
                    }
                    Err(e) => tracing::debug!(%e, "card still gone; will retry"),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_device(
    card: &Duplex,
    engine: &mut GraphEngine,
    routing: &Arc<Mutex<Routing>>,
    shared: &Arc<RtShared>,
    input: &mut [f32],
    output: &mut [f32],
    scratch32: (&mut [i32], &mut [i32]),
    scratch16: (&mut [i16], &mut [i16]),
) -> Exit {
    // The `IO` handles live in THIS frame because `alsa::IO` borrows its
    // `PCM` and the crate panics if a second one is made while one is
    // alive. A stored pair is therefore not expressible, and reopening
    // means rebuilding both `PCM`s from scratch — do not "tidy" these into
    // struct fields.
    let (in32, out32) = scratch32;
    let (in16, out16) = scratch16;
    let io_in32 = card.capture.io_i32();
    let io_out32 = card.playback.io_i32();
    let io_in16 = card.capture.io_i16();
    let io_out16 = card.playback.io_i16();

    let period = card.period;
    let in_frame = period * usize::from(card.in_channels);
    let out_frame = period * usize::from(card.out_channels);

    let mut worst = 0u32;
    let mut window = Instant::now();
    loop {
        if shared.stop.load(Ordering::Relaxed) {
            return Exit::Shutdown;
        }
        // Here rather than once before the loop, because recovery leaves
        // the pair prepared and stopped, and every restart needs the same
        // priming as the first.
        if let Err(errno) = start_linked(card, &io_out32, &io_out16, out32, out16) {
            match recover(card, errno, shared) {
                Ok(()) => continue,
                Err(why) => return Exit::Dead(why),
            }
        }
        let read = match card.format {
            SampleFormat::S32 => io_in32
                .as_ref()
                .map_err(|e| Errno::from_raw_os_error(e.errno()))
                .and_then(|io| {
                    io.readi(&mut in32[..in_frame])
                        .map_err(|e| Errno::from_raw_os_error(e.errno()))
                }),
            SampleFormat::S16 => io_in16
                .as_ref()
                .map_err(|e| Errno::from_raw_os_error(e.errno()))
                .and_then(|io| {
                    io.readi(&mut in16[..in_frame])
                        .map_err(|e| Errno::from_raw_os_error(e.errno()))
                }),
        };
        let frames = match read {
            Ok(frames) => frames,
            Err(errno) => match recover(card, errno, shared) {
                Ok(()) => continue,
                Err(why) => return Exit::Dead(why),
            },
        };

        let started = Instant::now();
        let route = *routing.lock().expect("routing lock");
        // De-interleave straight into the engine frame. No ring and no
        // assembler: there is exactly one capture device and it is on this
        // thread, so a ring would buy a copy and nothing else.
        input[..frames * MAX_INPUT_CHANNELS].fill(0.0);
        if route.input_open {
            let offset = usize::from(route.input_offset);
            let channels = usize::from(card.in_channels);
            for f in 0..frames {
                for c in 0..channels.min(MAX_INPUT_CHANNELS - offset) {
                    input[f * MAX_INPUT_CHANNELS + offset + c] = match card.format {
                        SampleFormat::S32 => from_s32(in32[f * channels + c]),
                        SampleFormat::S16 => from_s16(in16[f * channels + c]),
                    };
                }
            }
        }

        engine.process(
            &input[..frames * MAX_INPUT_CHANNELS],
            MAX_INPUT_CHANNELS,
            &mut output[..frames * MAX_OUTPUT_CHANNELS],
        );

        // The card's first channels are the monitor; the rest come from
        // wherever the orchestrator put this device in the plane. With the
        // monitor at plane 0/1 and the slot at offset 2, that is the
        // identity map — but it is written generally so it does not
        // silently depend on the coincidence.
        let channels = usize::from(card.out_channels);
        let monitor = usize::from(MONITOR_CHANNELS).min(channels);
        let offset = usize::from(route.output_offset);
        for f in 0..frames {
            let plane = f * MAX_OUTPUT_CHANNELS;
            for c in 0..channels {
                let value = if c < monitor {
                    output[plane + c]
                } else if route.output_open {
                    let index = plane + offset + (c - monitor);
                    if index < plane + MAX_OUTPUT_CHANNELS {
                        output[index]
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                match card.format {
                    SampleFormat::S32 => out32[f * channels + c] = to_s32(value),
                    SampleFormat::S16 => out16[f * channels + c] = to_s16(value),
                }
            }
        }

        let written = match card.format {
            SampleFormat::S32 => io_out32
                .as_ref()
                .map_err(|e| Errno::from_raw_os_error(e.errno()))
                .and_then(|io| {
                    io.writei(&out32[..frames * channels])
                        .map_err(|e| Errno::from_raw_os_error(e.errno()))
                }),
            SampleFormat::S16 => io_out16
                .as_ref()
                .map_err(|e| Errno::from_raw_os_error(e.errno()))
                .and_then(|io| {
                    io.writei(&out16[..frames * channels])
                        .map_err(|e| Errno::from_raw_os_error(e.errno()))
                }),
        };
        if let Err(errno) = written {
            match recover(card, errno, shared) {
                Ok(()) => continue,
                Err(why) => return Exit::Dead(why),
            }
        }
        let _ = out_frame;

        // Two `Instant::now()` calls a block, published once a second.
        // This is the number that decides whether a period size is
        // achievable, and it is measured rather than estimated because a
        // latency figure nobody measured is a wish.
        worst = worst.max(started.elapsed().as_micros().min(u128::from(u32::MAX)) as u32);
        if window.elapsed() >= Duration::from_secs(1) {
            shared.worst_block_us.store(worst, Ordering::Relaxed);
            worst = 0;
            window = Instant::now();
        }
    }
}

fn errno(e: alsa::Error) -> Errno {
    Errno::from_raw_os_error(e.errno())
}

/// Bring the prepared, linked pair up without an instant underrun.
///
/// The two PCMs are linked, so starting capture starts playback too — and
/// a playback ring that starts EMPTY underruns on its first period, which
/// XRUNs both directions, which the read side reports as `EPIPE`, which
/// recovery answers by starting again, empty. Measured on this machine:
/// ~400,000 xruns per second of exactly that, with the engine never
/// rendering a block. So playback is primed with [`PERIODS`] periods of
/// silence first; the last write crosses its start threshold and starts
/// both directions with a full ring. A no-op while already running.
fn start_linked(
    card: &Duplex,
    io_out32: &Result<alsa::pcm::IO<'_, i32>, alsa::Error>,
    io_out16: &Result<alsa::pcm::IO<'_, i16>, alsa::Error>,
    out32: &mut [i32],
    out16: &mut [i16],
) -> Result<(), Errno> {
    if card.capture.state() == State::Running {
        return Ok(());
    }
    // After an xrun the playback side may still be in the XRUN state;
    // writing to it would only report the xrun again.
    if !matches!(card.playback.state(), State::Prepared | State::Running) {
        card.playback.prepare().map_err(errno)?;
    }
    let frames = card.period * usize::from(card.out_channels);
    for _ in 0..PERIODS {
        match card.format {
            SampleFormat::S32 => {
                out32[..frames].fill(0);
                io_out32
                    .as_ref()
                    .map_err(|e| errno(alsa::Error::new("io", e.errno())))?
                    .writei(&out32[..frames])
                    .map_err(errno)?;
            }
            SampleFormat::S16 => {
                out16[..frames].fill(0);
                io_out16
                    .as_ref()
                    .map_err(|e| errno(alsa::Error::new("io", e.errno())))?
                    .writei(&out16[..frames])
                    .map_err(errno)?;
            }
        }
    }
    if card.capture.state() != State::Running {
        card.capture.start().map_err(errno)?;
    }
    Ok(())
}

/// Try to bring the card back. `Err` means it is not coming back. On
/// `Ok` the pair is prepared and stopped; the loop head starts it again,
/// primed.
fn recover(card: &Duplex, errno: Errno, shared: &Arc<RtShared>) -> Result<(), String> {
    match errno {
        // An xrun: recoverable in place, costs one block, never the take.
        Errno::PIPE => {
            shared.xruns.fetch_add(1, Ordering::Relaxed);
            shared.underruns.fetch_add(1, Ordering::Relaxed);
            card.capture
                .try_recover(alsa::Error::unsupported("xrun"), true)
                .or_else(|_| card.capture.prepare())
                .map_err(|e| format!("xrun recovery failed: {e}"))?;
            Ok(())
        }
        // Suspended (a laptop lid, a USB power event): resume, then
        // re-prepare. Bounded, because a card that will not resume is a
        // card that is gone.
        Errno::STRPIPE => {
            for _ in 0..50 {
                match card.playback.resume() {
                    Ok(()) => return Ok(()),
                    Err(e) if Errno::from_raw_os_error(e.errno()) == Errno::AGAIN => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => break,
                }
            }
            card.capture
                .prepare()
                .map_err(|e| format!("resume failed: {e}"))?;
            Ok(())
        }
        other => Err(format!("{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_samples_clip_rather_than_wrapping() {
        // Wraparound at full scale is the loudest sound a PA can make, and
        // it is one missing clamp away in every conversion below.
        // Positive full scale saturates one below the rail and negative
        // reaches it — the ordinary asymmetry of two's complement, not a
        // bug. What matters is that nothing WRAPS: a +2.0 sample must not
        // come out as a large negative one.
        assert_eq!(to_s32(1.0), i32::MAX);
        assert_eq!(to_s32(2.0), i32::MAX);
        assert_eq!(to_s32(-1.0), i32::MIN);
        assert_eq!(to_s32(-2.0), i32::MIN);
        assert_eq!(to_s16(4.0), i16::MAX);
        assert_eq!(to_s16(-4.0), i16::MIN);
        assert!(to_s32(2.0) > 0 && to_s16(4.0) > 0, "no wraparound");
    }

    #[test]
    fn conversion_round_trips_within_a_quantisation_step() {
        for value in [-0.9_f32, -0.25, 0.0, 0.25, 0.9] {
            assert!((from_s32(to_s32(value)) - value).abs() < 1e-6, "{value}");
            assert!((from_s16(to_s16(value)) - value).abs() < 1e-4, "{value}");
        }
    }

    #[test]
    fn silence_converts_to_silence_in_both_widths() {
        // The obvious one, and the one that breaks if a scale factor is
        // ever written as an offset.
        assert_eq!(to_s32(0.0), 0);
        assert_eq!(to_s16(0.0), 0);
        assert_eq!(from_s32(0), 0.0);
        assert_eq!(from_s16(0), 0.0);
    }

    #[test]
    fn card_health_never_holds_a_card_and_an_error_at_once() {
        let card = OpenedCard {
            name: "hw:1".into(),
            in_channels: 10,
            out_channels: 12,
        };
        let mut health = CardHealth::default();
        assert_eq!(health.generation, 0);

        health.went_down("hw:0 (Capture): No such file or directory".into());
        assert!(health.opened.is_none());
        assert!(
            health
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("hw:0"))
        );
        assert_eq!(health.generation, 1);

        health.came_up(card.clone());
        assert_eq!(health.opened.as_ref(), Some(&card));
        assert!(
            health.last_error.is_none(),
            "coming up clears the reason it was down"
        );
        assert_eq!(health.generation, 2);

        health.went_down("EPIPE".into());
        assert!(health.opened.is_none(), "going down clears the card");
        assert_eq!(
            health.generation, 3,
            "every transition moves the generation"
        );
    }

    /// Needs libasound (CI installs it) but no hardware: `hw:99` does not
    /// exist anywhere, so the open fails deterministically.
    #[test]
    fn a_card_that_will_not_open_is_not_a_boot_error() {
        use trib_core::MixerState;
        use trib_engine::{InputSlots, OutputSlots, compile, engine_pair};

        let compiled = compile(
            &MixerState::default(),
            48_000,
            256,
            &InputSlots::default(),
            &OutputSlots::with_monitor(),
        );
        let (_handle, engine) = engine_pair(compiled.graph);
        let backend = AlsaBackend::new(Some("hw:99".into()));
        let stream = backend
            .start(
                &StreamConfig {
                    sample_rate: 48_000,
                    block_size: 256,
                },
                engine,
            )
            .expect("a missing card starts the timer clock, it does not refuse to start");

        assert!(
            backend.input_devices().is_empty(),
            "nothing to patch while the card is down"
        );
        assert!(backend.output_devices().is_empty());
        let status = backend.status();
        assert!(!status.running);
        assert_eq!(status.card, None);
        assert!(
            status.error.as_deref().is_some_and(|e| e.contains("hw:99")),
            "the reason names the card: {status:?}"
        );
        assert_eq!(
            backend.generation(),
            1,
            "the failed boot open is a transition"
        );
        drop(stream);
    }

    #[test]
    fn a_second_card_is_refused_with_the_reason() {
        let error = second_card_error("hw:1");
        let text = error.to_string();
        assert!(text.contains("hw:1"));
        assert!(
            text.contains("own clock"),
            "the refusal has to say WHY, or it reads as a missing feature"
        );
    }
}
