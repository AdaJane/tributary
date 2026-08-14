//! Real device IO via cpal (ALSA/PipeWire on Linux). cpal has no duplex
//! stream and its streams are !Send, so a dedicated thread owns them all:
//! the output stream plus one input stream per device, opened and closed
//! on command. The engine is clocked by its OWN timer thread — never by a
//! device callback — rendering into a ring the output callback drains, so
//! a stalling output device (Bluetooth renegotiation, route changes) costs
//! monitor audio only, never metering or a take. A boot without any usable
//! output is tolerated the same way: the stream thread starts regardless
//! and keeps rebuilding on the current default. Each input feeds its own
//! ring; the engine thread's assembler merges them into the fixed
//! `MAX_INPUT_CHANNELS` engine frame. Devices open at the engine rate —
//! PipeWire/ALSA adapt or the open fails and the jacks read silence.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::RingBuffer;
use trib_engine::{GraphEngine, MAX_INPUT_CHANNELS, MAX_OUTPUT_CHANNELS, MONITOR_OUT};

use crate::assembler::{
    ASSEMBLER_RING_CAPACITY, AssemblerCmd, Attached, InputAssembler, InputShared, push_frames,
};
use crate::backend::{
    AudioBackend, AudioError, CardInfo, CardProfile, InputDeviceInfo, InputStreamStatus, OpenInput,
    OpenOutput, OutputDeviceInfo, OutputStreamStatus, StreamConfig, StreamHandle,
};
use crate::disassembler::{
    AttachedOutput, DISASSEMBLER_RING_CAPACITY, DisassemblerCmd, OutputDisassembler, OutputShared,
};
use crate::stall::{StallWatch, Transition};

/// Input ring headroom in blocks: enough for scheduling jitter between an
/// input callback and the output callback without meaningful latency.
const INPUT_RING_BLOCKS: usize = 16;

/// How long the stream thread waits for a command before housekeeping
/// (retire-ring drain, stop-flag check).
const COMMAND_POLL: Duration = Duration::from_millis(100);

pub struct CpalBackend;

enum StreamCmd {
    OpenInput {
        req: OpenInput,
        reply: Sender<Result<(), AudioError>>,
    },
    CloseInput {
        device: Option<String>,
        reply: Sender<Result<(), AudioError>>,
    },
    OpenOutput {
        req: OpenOutput,
        reply: Sender<Result<(), AudioError>>,
    },
    CloseOutput {
        device: Option<String>,
        reply: Sender<Result<(), AudioError>>,
    },
    Shutdown,
}

/// Status entries shared between the stream thread (writes) and the
/// handle (reads). Keyed by device identity.
type StatusMap = Arc<Mutex<HashMap<Option<String>, (u16, u16, Arc<InputShared>)>>>;

/// The output side's equivalent, keyed the same way.
type OutStatusMap = Arc<Mutex<HashMap<Option<String>, (u16, u16, Arc<OutputShared>)>>>;

struct CpalStream {
    cmd_tx: Sender<StreamCmd>,
    status: StatusMap,
    out_status: OutStatusMap,
    thread: Option<JoinHandle<()>>,
}

impl StreamHandle for CpalStream {
    fn open_input(&self, req: OpenInput) -> Result<(), AudioError> {
        let (reply, response) = channel();
        self.cmd_tx
            .send(StreamCmd::OpenInput { req, reply })
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?;
        response
            .recv()
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?
    }

    fn close_input(&self, device: Option<&str>) -> Result<(), AudioError> {
        let (reply, response) = channel();
        self.cmd_tx
            .send(StreamCmd::CloseInput {
                device: device.map(str::to_owned),
                reply,
            })
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?;
        response
            .recv()
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?
    }

    fn input_status(&self) -> Vec<InputStreamStatus> {
        let status = self.status.lock().expect("status map lock");
        status
            .iter()
            .map(|(device, (offset, channels, shared))| InputStreamStatus {
                device: device.clone(),
                offset: *offset,
                channels: *channels,
                failed: shared.failed.load(Ordering::Relaxed),
                underruns: shared.underruns.load(Ordering::Relaxed),
                overruns: shared.overruns.load(Ordering::Relaxed),
            })
            .collect()
    }

    fn open_output(&self, req: OpenOutput) -> Result<(), AudioError> {
        let (reply, response) = channel();
        self.cmd_tx
            .send(StreamCmd::OpenOutput { req, reply })
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?;
        response
            .recv()
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?
    }

    fn close_output(&self, device: Option<&str>) -> Result<(), AudioError> {
        let (reply, response) = channel();
        self.cmd_tx
            .send(StreamCmd::CloseOutput {
                device: device.map(str::to_owned),
                reply,
            })
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?;
        response
            .recv()
            .map_err(|_| AudioError::Stream("audio thread gone".into()))?
    }

    fn output_status(&self) -> Vec<OutputStreamStatus> {
        let status = self.out_status.lock().expect("output status map lock");
        status
            .iter()
            .map(|(device, (offset, channels, shared))| OutputStreamStatus {
                device: device.clone(),
                offset: *offset,
                channels: *channels,
                failed: shared.failed.load(Ordering::Relaxed),
                underruns: shared.underruns.load(Ordering::Relaxed),
                overruns: shared.overruns.load(Ordering::Relaxed),
                // Only the real-time backend opens a `hw:` PCM, so only it
                // can see an xrun or time its own render loop.
                xruns: 0,
                worst_block_us: 0,
            })
            .collect()
    }
}

impl Drop for CpalStream {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(StreamCmd::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl AudioBackend for CpalBackend {
    fn name(&self) -> &'static str {
        "cpal"
    }

    /// The system's input sources under their friendly names — the SAME
    /// list the OS sound menu shows — via the Pulse layer. Without a
    /// Pulse/PipeWire server, raw ALSA devices are the fallback.
    fn input_devices(&self) -> Vec<InputDeviceInfo> {
        if let Some(sources) = crate::pulse::enumerate_sources() {
            return sources
                .into_iter()
                .map(|s| InputDeviceInfo {
                    name: s.name,
                    description: Some(s.description),
                    channels: s.channels,
                    active: s.default,
                    pulse: true,
                    card: s.card,
                    channel_map: s.channel_map,
                    muted: s.muted,
                    volume_percent: s.volume_percent,
                })
                .collect();
        }
        // Once per boot: on a PipeWire appliance this fallback is a broken
        // state worth one loud line; repeating it per enumeration is noise.
        static PULSE_FELL_BACK: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !PULSE_FELL_BACK.swap(true, Ordering::Relaxed) {
            tracing::warn!("no Pulse/PipeWire server answered; enumerating raw ALSA devices");
        }
        let host = cpal::default_host();
        let default_name = host.default_input_device().and_then(|d| d.name().ok());
        let Ok(devices) = host.input_devices() else {
            return Vec::new();
        };
        devices
            .filter_map(|device| {
                let name = device.name().ok()?;
                // A device that won't state an input config can't be opened.
                let channels = device.default_input_config().ok()?.channels();
                Some(InputDeviceInfo {
                    active: default_name.as_deref() == Some(name.as_str()),
                    name,
                    description: None,
                    channels,
                    pulse: false,
                    // Raw ALSA has no card-profile concept to offer, and no
                    // server-side mute or volume: the mixer is the card's
                    // own business here.
                    card: None,
                    channel_map: None,
                    muted: false,
                    volume_percent: None,
                })
            })
            .collect()
    }

    fn supports_outputs(&self) -> bool {
        true
    }

    /// Output devices as ALSA names them.
    ///
    /// Deliberately NOT the Pulse sink list the input side uses. A sink
    /// name (`alsa_output.usb-…`) is not something cpal can open, and
    /// showing a name we cannot act on would be a worse lie than showing a
    /// plainer one: the name printed here is exactly the name
    /// `open_output` will pass to cpal. Alias devices ("default",
    /// "pipewire") fold together in the console the same way they do on
    /// the input board.
    ///
    /// `monitor_channels` is 0 for every one of them: this backend puts
    /// the control-room feed on its OWN cpal stream, so no device has to
    /// give up channels for it. The real-time backend, which shares one
    /// card between the monitor and everything else, answers 2.
    fn output_devices(&self) -> Vec<OutputDeviceInfo> {
        let host = cpal::default_host();
        let default_name = host.default_output_device().and_then(|d| d.name().ok());
        let Ok(devices) = host.output_devices() else {
            return Vec::new();
        };
        devices
            .filter_map(|device| {
                let name = device.name().ok()?;
                // A device that will not state an output config cannot be
                // opened, so offering it would be offering a dead jack.
                let channels = device.default_output_config().ok()?.channels();
                Some(OutputDeviceInfo {
                    active: default_name.as_deref() == Some(name.as_str()),
                    name,
                    description: None,
                    channels,
                    pulse: false,
                    card: None,
                    channel_map: None,
                    muted: false,
                    volume_percent: None,
                    monitor_channels: 0,
                })
            })
            .collect()
    }

    fn input_cards(&self) -> Vec<CardInfo> {
        crate::pulse::enumerate_cards()
            .unwrap_or_default()
            .into_iter()
            .map(|c| CardInfo {
                name: c.name,
                active_profile: c.active_profile,
                profiles: c
                    .profiles
                    .into_iter()
                    .map(|p| CardProfile {
                        name: p.name,
                        description: p.description,
                    })
                    .collect(),
            })
            .collect()
    }

    fn set_card_profile(&self, card: &str, profile: &str) -> Result<(), AudioError> {
        crate::pulse::set_card_profile(card, profile)
    }

    fn start(
        &self,
        config: &StreamConfig,
        engine: GraphEngine,
    ) -> Result<Box<dyn StreamHandle>, AudioError> {
        let config = *config;
        let (cmd_tx, cmd_rx) = channel::<StreamCmd>();
        let status: StatusMap = Arc::default();
        let status_thread = status.clone();
        let out_status: OutStatusMap = Arc::default();
        let out_status_thread = out_status.clone();
        let (setup_tx, setup_rx) = channel::<Result<(), AudioError>>();

        let thread = std::thread::Builder::new()
            .name("trib-cpal-audio".into())
            .spawn(move || {
                stream_thread(
                    config,
                    engine,
                    cmd_rx,
                    status_thread,
                    out_status_thread,
                    setup_tx,
                )
            })
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        match setup_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(CpalStream {
                cmd_tx,
                status,
                out_status,
                thread: Some(thread),
            })),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Stream("audio thread died during setup".into()))
            }
        }
    }
}

/// One open input and its assembler identity. Dropping the source stops
/// its feed (cpal: the callback and its ring producer die with the
/// stream; parec: the child is killed and its reader thread exits).
struct OpenedInput {
    _source: InputSource,
    offset: u16,
}

struct OpenedOutput {
    _stream: cpal::Stream,
    offset: u16,
}

enum InputSource {
    Cpal(#[allow(dead_code)] cpal::Stream),
    Pulse(#[allow(dead_code)] crate::pulse::ParecCapture),
}

/// The !Send island: owns the output stream, every input stream, and the
/// control-side ends of the assembler rings. The ENGINE does not run here
/// or in the output callback — it runs on its own timer-clocked thread
/// (below), so a stalling output device (a Bluetooth sink renegotiating,
/// a route change) can never freeze metering or recording. The output
/// callback only drains the engine's ring; an outage costs monitor audio
/// and nothing else.
fn stream_thread(
    config: StreamConfig,
    engine: GraphEngine,
    cmd_rx: Receiver<StreamCmd>,
    status: StatusMap,
    out_status: OutStatusMap,
    setup_tx: Sender<Result<(), AudioError>>,
) {
    let (asm_cmd_tx, asm_cmd_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
    let (asm_retire_tx, mut asm_retire_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
    let assembler = InputAssembler::new(asm_cmd_rx, asm_retire_tx);

    // The output half, built the same way and for the same reasons. It
    // starts with nothing attached — patched outputs open on demand — and
    // costs one empty slot-table walk per block until then.
    let (dis_cmd_tx, dis_cmd_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
    let (dis_retire_tx, mut dis_retire_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
    let disassembler = OutputDisassembler::new(dis_cmd_rx, dis_retire_tx);

    let (out_tx, out_rx) = RingBuffer::<f32>::new(OUTPUT_RING_FRAMES * 2);
    // Mutex so a wedged stream can be dropped and rebuilt around the same
    // ring. Uncontended except during a rebuild; the callback try_locks
    // and pours silence rather than ever waiting.
    let out_rx = Arc::new(Mutex::new(out_rx));
    let beat = Arc::new(AtomicU64::new(0));

    // The engine clock IS the backend; setup fails only if it can't run.
    let engine_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let engine_thread = {
        let stop = engine_stop.clone();
        std::thread::Builder::new()
            .name("trib-engine-clock".into())
            .spawn(move || engine_clock(config, engine, assembler, disassembler, out_tx, stop))
    };
    let engine_thread = match engine_thread {
        Ok(handle) => handle,
        Err(e) => {
            let _ = setup_tx.send(Err(AudioError::Stream(format!("engine clock: {e}"))));
            return;
        }
    };
    let _ = setup_tx.send(Ok(()));

    // The monitor output is best-effort: without a usable sink, inputs,
    // meters, and recording still run; the rebuild loop below keeps
    // trying the current default.
    let mut output_stream = match build_output(&config, &out_rx, &beat) {
        Ok(stream) => Some(stream),
        Err(e) => {
            tracing::error!(
                %e,
                "no usable monitor output — starting WITHOUT monitor audio; \
                 inputs, metering and recording run; retrying on the default sink"
            );
            None
        }
    };

    let mut inputs: HashMap<Option<String>, OpenedInput> = HashMap::new();
    let mut outputs: HashMap<Option<String>, OpenedOutput> = HashMap::new();
    let mut asm_cmd_tx = asm_cmd_tx;
    let mut dis_cmd_tx = dis_cmd_tx;
    let mut watch = StallWatch::new(Instant::now());
    let mut stalled_since: Option<Instant> = None;
    loop {
        let now = Instant::now();
        match watch.observe(beat.load(Ordering::Relaxed), now) {
            Some(Transition::Stalled) => {
                stalled_since = Some(now);
                if output_stream.is_some() {
                    tracing::warn!(
                        "monitor output stalled (routing change?) — engine unaffected, \
                         meters and recording continue; monitor audio is out until it resumes"
                    );
                } else {
                    // Never built: the boot error already said so loudly.
                    tracing::debug!("still no monitor output; rebuild pending");
                }
            }
            Some(Transition::Resumed(outage)) => {
                stalled_since = None;
                tracing::info!(outage_secs = outage.as_secs_f32(), "monitor output resumed");
            }
            None => {}
        }
        // A stream that stays wedged (its device vanished mid-stream) — or
        // was never built at boot — never resumes by itself; rebuild on the
        // current default, retrying every window until one sticks. The
        // command loop's cadence is untouched.
        if stalled_since.is_some_and(|since| now.duration_since(since) >= REBUILD_AFTER) {
            drop(output_stream.take());
            // The stall left the ring full of stale audio; drop all but the
            // prefill so the rebuilt monitor starts near-live.
            if let Ok(mut rx) = out_rx.lock() {
                while rx.slots() > OUTPUT_PREFILL_FRAMES * 2 {
                    let _ = rx.pop();
                }
            }
            match build_output(&config, &out_rx, &beat) {
                Ok(stream) => {
                    output_stream = Some(stream);
                    tracing::info!("monitor output rebuilt on the current default device");
                }
                // The stall/boot failure was already announced; retries are
                // routine until a sink appears.
                Err(e) => tracing::debug!(%e, "monitor output rebuild failed; will retry"),
            }
            stalled_since = Some(Instant::now());
        }
        match cmd_rx.recv_timeout(COMMAND_POLL) {
            Ok(StreamCmd::OpenInput { req, reply }) => {
                let result = open_input(&config, &req, &mut asm_cmd_tx, &status, &mut inputs);
                let _ = reply.send(result);
            }
            Ok(StreamCmd::CloseInput { device, reply }) => {
                let result = if let Some(opened) = inputs.remove(&device) {
                    // Drop the stream FIRST: the ring producer dies with
                    // its callback, so the retired consumer is the last
                    // owner and its drop (here, next drain) frees the ring.
                    let offset = opened.offset;
                    drop(opened);
                    status.lock().expect("status map lock").remove(&device);
                    asm_cmd_tx
                        .push(AssemblerCmd::Detach { offset })
                        .map_err(|_| AudioError::Stream("assembler ring full".into()))
                } else {
                    Ok(()) // already closed: idempotent
                };
                let _ = reply.send(result);
            }
            Ok(StreamCmd::OpenOutput { req, reply }) => {
                let result = open_output(&config, &req, &mut dis_cmd_tx, &out_status, &mut outputs);
                let _ = reply.send(result);
            }
            Ok(StreamCmd::CloseOutput { device, reply }) => {
                let result = if let Some(opened) = outputs.remove(&device) {
                    // Drop the stream FIRST, exactly as on the input side:
                    // the ring consumer dies with its callback, so the
                    // retired producer is the last owner.
                    let offset = opened.offset;
                    drop(opened);
                    out_status
                        .lock()
                        .expect("output status map lock")
                        .remove(&device);
                    dis_cmd_tx
                        .push(DisassemblerCmd::Detach { offset })
                        .map_err(|_| AudioError::Stream("disassembler ring full".into()))
                } else {
                    Ok(()) // already closed: idempotent
                };
                let _ = reply.send(result);
            }
            Ok(StreamCmd::Shutdown) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Retired consumers drop here — control side, never the callback.
        while asm_retire_rx.pop().is_ok() {}
        while dis_retire_rx.pop().is_ok() {}
    }
    drop(output_stream);
    drop(inputs);
    drop(outputs);
    engine_stop.store(true, Ordering::Relaxed);
    let _ = engine_thread.join();
    // The assembler (now dropped with the engine thread) retired any
    // remaining attached consumers; drain them here, control side.
    while asm_retire_rx.pop().is_ok() {}
}

/// How long a monitor stall may last before the stream is torn down and
/// rebuilt on the current default device.
const REBUILD_AFTER: Duration = Duration::from_secs(5);

/// The output callback: drains the engine's ring, pouring silence on
/// underrun or while a rebuild holds the lock. Shared so a rebuilt stream
/// picks up the same ring and beat counter.
fn drain_closure(
    out_rx: &Arc<Mutex<rtrb::Consumer<f32>>>,
    beat: &Arc<AtomicU64>,
) -> impl FnMut(&mut [f32]) + Send + 'static {
    let out_rx = out_rx.clone();
    let beat = beat.clone();
    move |data| {
        beat.fetch_add(1, Ordering::Relaxed);
        match out_rx.try_lock() {
            Ok(mut rx) => {
                for sample in data.iter_mut() {
                    *sample = rx.pop().unwrap_or(0.0);
                }
            }
            Err(_) => data.fill(0.0),
        }
    }
}

/// How much rendered audio the output ring can hold. Capacity bounds a
/// stalled sink's backlog; the steady-state fill (= monitor latency) is
/// set by the prefill below, not by this.
const OUTPUT_RING_FRAMES: usize = 8192;

/// Silence written ahead of the first block so the sink's largest burst
/// never outruns the engine's cadence: ~43 ms of monitor latency at 48 kHz,
/// bought once, in exchange for an engine no output device can stall.
const OUTPUT_PREFILL_FRAMES: usize = 2048;

/// The engine's own clock: block-cadence rendering on absolute deadlines
/// (sleep-until, so scheduling jitter never accumulates), exactly the
/// FakeBackend pattern. Pushes to a full ring are dropped — a stalled sink
/// costs monitor samples, never a frozen take.
fn engine_clock(
    config: StreamConfig,
    mut engine: GraphEngine,
    mut assembler: InputAssembler,
    mut disassembler: OutputDisassembler,
    mut out_tx: rtrb::Producer<f32>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let frames = config.block_size;
    let block = Duration::from_secs_f64(frames as f64 / config.sample_rate as f64);
    let mut input = vec![0.0f32; frames * MAX_INPUT_CHANNELS];
    let mut output = trib_engine::output_buffer(frames);
    for _ in 0..OUTPUT_PREFILL_FRAMES * 2 {
        let _ = out_tx.push(0.0);
    }
    let mut next = Instant::now() + block;
    while !stop.load(Ordering::Relaxed) {
        assembler.drain_commands();
        assembler.fill(&mut input, frames);
        disassembler.drain_commands();
        engine.process(&input, MAX_INPUT_CHANNELS, &mut output);
        // Patched outputs take their own slices of the plane. Done before
        // the monitor drain below so a slow monitor sink cannot delay a
        // direct out.
        disassembler.scatter(&output, frames);
        // The monitor sink takes the reserved pair only. The rest of the
        // plane belongs to direct outs, which leave through their own
        // devices and must never be folded into the control-room feed.
        for frame in output.chunks_exact(MAX_OUTPUT_CHANNELS) {
            let _ = out_tx.push(frame[MONITOR_OUT[0] as usize]);
            let _ = out_tx.push(frame[MONITOR_OUT[1] as usize]);
        }
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > block * 8 {
            // A suspend or debugger pause: rebase rather than sprint.
            next = now;
        }
        next += block;
    }
}

/// Start draining a slice of the engine plane to one output device.
///
/// Unlike the input side, this does NOT get a Pulse variant. Inputs go
/// through `parec` because `libpulse-dev` is absent and `set_env` is unsafe
/// under edition 2024; outputs have no such constraint, so cpal opens them
/// directly. That also means the mapping is ALSA-positional by index,
/// which sidesteps the whole `--no-remap` class of bug the capture path had
/// to learn about the hard way — the server never gets to re-order these.
fn open_output(
    config: &StreamConfig,
    req: &OpenOutput,
    dis_cmd_tx: &mut rtrb::Producer<DisassemblerCmd>,
    status: &OutStatusMap,
    outputs: &mut HashMap<Option<String>, OpenedOutput>,
) -> Result<(), AudioError> {
    if outputs.contains_key(&req.device) {
        return Ok(()); // already open: idempotent
    }
    let channels = req.channels;
    let shared = Arc::new(OutputShared::default());
    let host = cpal::default_host();
    let device = match &req.device {
        None => host
            .default_output_device()
            .ok_or_else(|| AudioError::Device("no default output device".into()))?,
        Some(name) => host
            .output_devices()
            .map_err(|e| AudioError::Device(e.to_string()))?
            .find(|d| d.name().is_ok_and(|n| &n == name))
            .ok_or_else(|| AudioError::Device(format!("no output device named {name:?}")))?,
    };

    let try_build =
        |buffer_size: cpal::BufferSize| -> Result<(cpal::Stream, rtrb::Producer<f32>), AudioError> {
            let (tx, mut rx) = RingBuffer::<f32>::new(
                config.block_size * usize::from(channels) * INPUT_RING_BLOCKS,
            );
            let error_shared = shared.clone();
            let cb_shared = shared.clone();
            let stream = device
                .build_output_stream(
                    &cpal::StreamConfig {
                        channels,
                        sample_rate: cpal::SampleRate(config.sample_rate),
                        buffer_size,
                    },
                    move |data: &mut [f32], _| {
                        let mut starved = 0u64;
                        for sample in data.iter_mut() {
                            *sample = match rx.pop() {
                                Ok(value) => value,
                                Err(_) => {
                                    starved += 1;
                                    0.0
                                }
                            };
                        }
                        if starved > 0 {
                            cb_shared.underruns.fetch_add(starved, Ordering::Relaxed);
                        }
                    },
                    move |e| {
                        tracing::error!(%e, "output stream error");
                        error_shared.failed.store(true, Ordering::Relaxed);
                    },
                    None,
                )
                .map_err(|e| AudioError::Stream(format!("output: {e}")))?;
            Ok((stream, tx))
        };
    // Raw ALSA devices often refuse a fixed buffer size; the engine
    // sub-blocks anyway, so Default is a fine second try.
    let (stream, producer) = try_build(cpal::BufferSize::Fixed(config.block_size as u32))
        .or_else(|_| try_build(cpal::BufferSize::Default))?;
    stream
        .play()
        .map_err(|e| AudioError::Stream(format!("output play: {e}")))?;

    dis_cmd_tx
        .push(DisassemblerCmd::Attach(Box::new(AttachedOutput {
            producer,
            offset: req.offset,
            channels,
            shared: shared.clone(),
        })))
        .map_err(|_| AudioError::Stream("disassembler ring full".into()))?;
    status
        .lock()
        .expect("output status map lock")
        .insert(req.device.clone(), (req.offset, channels, shared));
    outputs.insert(
        req.device.clone(),
        OpenedOutput {
            _stream: stream,
            offset: req.offset,
        },
    );
    Ok(())
}

fn open_input(
    config: &StreamConfig,
    req: &OpenInput,
    asm_cmd_tx: &mut rtrb::Producer<AssemblerCmd>,
    status: &StatusMap,
    inputs: &mut HashMap<Option<String>, OpenedInput>,
) -> Result<(), AudioError> {
    if inputs.contains_key(&req.device) {
        return Ok(()); // already open: idempotent
    }
    let channels = req.channels;
    let shared = Arc::new(InputShared::default());

    // Pulse sources open by NAME through parec — the server resamples to
    // the engine rate and the friendly system-menu list stays honest.
    if req.pulse {
        let (tx, rx) =
            RingBuffer::<f32>::new(config.block_size * usize::from(channels) * INPUT_RING_BLOCKS);
        let capture = crate::pulse::ParecCapture::spawn(
            req.device.as_deref(),
            channels,
            config.sample_rate,
            req.channel_map.as_deref(),
            tx,
            shared.clone(),
        )?;
        attach(
            asm_cmd_tx,
            status,
            inputs,
            req,
            InputSource::Pulse(capture),
            rx,
            shared,
        )?;
        return Ok(());
    }

    let host = cpal::default_host();
    let device = match &req.device {
        None => host
            .default_input_device()
            .ok_or_else(|| AudioError::Device("no default input device".into()))?,
        Some(name) => host
            .input_devices()
            .map_err(|e| AudioError::Device(e.to_string()))?
            .find(|d| d.name().is_ok_and(|n| &n == name))
            .ok_or_else(|| AudioError::Device(format!("no input device named {name:?}")))?,
    };
    let try_build =
        |buffer_size: cpal::BufferSize| -> Result<(cpal::Stream, rtrb::Consumer<f32>), AudioError> {
            let (mut tx, rx) = RingBuffer::<f32>::new(
                config.block_size * usize::from(channels) * INPUT_RING_BLOCKS,
            );
            let error_shared = shared.clone();
            let cb_shared = shared.clone();
            let stream = device
                .build_input_stream(
                    &cpal::StreamConfig {
                        channels,
                        sample_rate: cpal::SampleRate(config.sample_rate),
                        buffer_size,
                    },
                    move |data: &[f32], _| {
                        // Whole interleaved device frames; the assembler
                        // re-homes them at this device's offset. Frame
                        // -atomic on purpose: a torn frame rotates every
                        // one of this device's channels permanently.
                        let dropped = push_frames(&mut tx, data, usize::from(channels));
                        if dropped > 0 {
                            cb_shared.overruns.fetch_add(dropped, Ordering::Relaxed);
                        }
                    },
                    move |e| {
                        tracing::error!(%e, "input stream error");
                        error_shared.failed.store(true, Ordering::Relaxed);
                    },
                    None,
                )
                .map_err(|e| AudioError::Stream(format!("input: {e}")))?;
            Ok((stream, rx))
        };
    // Fixed block first (lowest jitter); raw hardware devices often refuse
    // it, and the ring absorbs whatever cadence they deliver instead.
    let (stream, rx) = try_build(cpal::BufferSize::Fixed(config.block_size as u32)).or_else(
        |fixed_err| {
            tracing::debug!(%fixed_err, "fixed buffer size refused; retrying with the device default");
            try_build(cpal::BufferSize::Default)
        },
    )?;
    stream
        .play()
        .map_err(|e| AudioError::Stream(format!("input play: {e}")))?;
    attach(
        asm_cmd_tx,
        status,
        inputs,
        req,
        InputSource::Cpal(stream),
        rx,
        shared,
    )
}

/// Hand a freshly opened source's ring to the assembler and book-keep it.
fn attach(
    asm_cmd_tx: &mut rtrb::Producer<AssemblerCmd>,
    status: &StatusMap,
    inputs: &mut HashMap<Option<String>, OpenedInput>,
    req: &OpenInput,
    source: InputSource,
    rx: rtrb::Consumer<f32>,
    shared: Arc<InputShared>,
) -> Result<(), AudioError> {
    asm_cmd_tx
        .push(AssemblerCmd::Attach(Box::new(Attached {
            consumer: rx,
            offset: req.offset,
            channels: req.channels,
            shared: shared.clone(),
        })))
        .map_err(|_| AudioError::Stream("assembler ring full".into()))?;
    status
        .lock()
        .expect("status map lock")
        .insert(req.device.clone(), (req.offset, req.channels, shared));
    inputs.insert(
        req.device.clone(),
        OpenedInput {
            _source: source,
            offset: req.offset,
        },
    );
    Ok(())
}

/// Build and start the output stream — the monitor sink. Its callback only
/// drains the engine's ring (`drain_closure`). Fixed block first (lowest
/// jitter); devices that refuse it get the default cadence, which the
/// prefill absorbs.
fn build_output(
    config: &StreamConfig,
    out_rx: &Arc<Mutex<rtrb::Consumer<f32>>>,
    beat: &Arc<AtomicU64>,
) -> Result<cpal::Stream, AudioError> {
    let host = cpal::default_host();
    let output_device = host
        .default_output_device()
        .ok_or_else(|| AudioError::Device("no default output device".into()))?;
    let try_build = |buffer_size: cpal::BufferSize| {
        let mut render = drain_closure(out_rx, beat);
        output_device.build_output_stream(
            &cpal::StreamConfig {
                channels: 2,
                sample_rate: cpal::SampleRate(config.sample_rate),
                buffer_size,
            },
            move |data: &mut [f32], _| render(data),
            |e| tracing::error!(%e, "output stream error"),
            None,
        )
    };
    let stream = try_build(cpal::BufferSize::Fixed(config.block_size as u32))
        .or_else(|fixed_err| {
            tracing::debug!(%fixed_err, "fixed buffer size refused; retrying with the device default");
            try_build(cpal::BufferSize::Default)
        })
        .map_err(|e| AudioError::Stream(format!("output: {e}")))?;
    stream
        .play()
        .map_err(|e| AudioError::Stream(format!("output play: {e}")))?;
    Ok(stream)
}
