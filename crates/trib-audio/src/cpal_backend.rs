//! Real device IO via cpal (ALSA/PipeWire on Linux). cpal has no duplex
//! stream and its streams are !Send, so a dedicated thread owns them all:
//! the output stream plus one input stream per device, opened and closed
//! on command. The engine is clocked by its OWN timer thread — never by a
//! device callback — rendering into a ring the output callback drains, so
//! a stalling output device (Bluetooth renegotiation, route changes) costs
//! monitor audio only, never metering or a take. Each input feeds its own
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
use trib_engine::{GraphEngine, MAX_INPUT_CHANNELS};

use crate::assembler::{
    ASSEMBLER_RING_CAPACITY, AssemblerCmd, Attached, InputAssembler, InputShared,
};
use crate::backend::{
    AudioBackend, AudioError, InputDeviceInfo, InputStreamStatus, OpenInput, StreamConfig,
    StreamHandle,
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
    Shutdown,
}

/// Status entries shared between the stream thread (writes) and the
/// handle (reads). Keyed by device identity.
type StatusMap = Arc<Mutex<HashMap<Option<String>, (u16, u16, Arc<InputShared>)>>>;

struct CpalStream {
    cmd_tx: Sender<StreamCmd>,
    status: StatusMap,
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
                })
                .collect();
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
                })
            })
            .collect()
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
        let (setup_tx, setup_rx) = channel::<Result<(), AudioError>>();

        let thread = std::thread::Builder::new()
            .name("trib-cpal-audio".into())
            .spawn(move || stream_thread(config, engine, cmd_rx, status_thread, setup_tx))
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        match setup_rx.recv() {
            Ok(Ok(())) => Ok(Box::new(CpalStream {
                cmd_tx,
                status,
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
    setup_tx: Sender<Result<(), AudioError>>,
) {
    let (asm_cmd_tx, asm_cmd_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
    let (asm_retire_tx, mut asm_retire_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
    let assembler = InputAssembler::new(asm_cmd_rx, asm_retire_tx);

    let (out_tx, out_rx) = RingBuffer::<f32>::new(OUTPUT_RING_FRAMES * 2);
    // Mutex so a wedged stream can be dropped and rebuilt around the same
    // ring. Uncontended except during a rebuild; the callback try_locks
    // and pours silence rather than ever waiting.
    let out_rx = Arc::new(Mutex::new(out_rx));
    let beat = Arc::new(AtomicU64::new(0));
    let mut output_stream = match build_output(&config, drain_closure(&out_rx, &beat)) {
        Ok(stream) => {
            let _ = setup_tx.send(Ok(()));
            Some(stream)
        }
        Err(e) => {
            let _ = setup_tx.send(Err(e));
            return;
        }
    };

    let engine_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let engine_thread = {
        let stop = engine_stop.clone();
        std::thread::Builder::new()
            .name("trib-engine-clock".into())
            .spawn(move || engine_clock(config, engine, assembler, out_tx, stop))
    };
    let engine_thread = match engine_thread {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!(%e, "engine clock thread failed to spawn");
            return;
        }
    };

    let mut inputs: HashMap<Option<String>, OpenedInput> = HashMap::new();
    let mut asm_cmd_tx = asm_cmd_tx;
    let mut watch = StallWatch::new(Instant::now());
    let mut stalled_since: Option<Instant> = None;
    loop {
        let now = Instant::now();
        match watch.observe(beat.load(Ordering::Relaxed), now) {
            Some(Transition::Stalled) => {
                stalled_since = Some(now);
                tracing::warn!(
                    "monitor output stalled (routing change?) — engine unaffected, \
                     meters and recording continue; monitor audio is out until it resumes"
                );
            }
            Some(Transition::Resumed(outage)) => {
                stalled_since = None;
                tracing::info!(outage_secs = outage.as_secs_f32(), "monitor output resumed");
            }
            None => {}
        }
        // A stream that stays wedged (its device vanished mid-stream) never
        // resumes by itself; rebuild on the current default, retrying every
        // window until one sticks. The command loop's cadence is untouched.
        if stalled_since.is_some_and(|since| now.duration_since(since) >= REBUILD_AFTER) {
            drop(output_stream.take());
            // The stall left the ring full of stale audio; drop all but the
            // prefill so the rebuilt monitor starts near-live.
            if let Ok(mut rx) = out_rx.lock() {
                while rx.slots() > OUTPUT_PREFILL_FRAMES * 2 {
                    let _ = rx.pop();
                }
            }
            match build_output(&config, drain_closure(&out_rx, &beat)) {
                Ok(stream) => {
                    output_stream = Some(stream);
                    tracing::info!("monitor output rebuilt on the current default device");
                }
                Err(e) => tracing::warn!(%e, "monitor output rebuild failed; will retry"),
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
            Ok(StreamCmd::Shutdown) => break,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Retired consumers drop here — control side, never the callback.
        while asm_retire_rx.pop().is_ok() {}
    }
    drop(output_stream);
    drop(inputs);
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
    mut out_tx: rtrb::Producer<f32>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let frames = config.block_size;
    let block = Duration::from_secs_f64(frames as f64 / config.sample_rate as f64);
    let mut input = vec![0.0f32; frames * MAX_INPUT_CHANNELS];
    let mut output = vec![0.0f32; frames * 2];
    for _ in 0..OUTPUT_PREFILL_FRAMES * 2 {
        let _ = out_tx.push(0.0);
    }
    let mut next = Instant::now() + block;
    while !stop.load(Ordering::Relaxed) {
        assembler.drain_commands();
        assembler.fill(&mut input, frames);
        engine.process(&input, MAX_INPUT_CHANNELS, &mut output);
        for &sample in &output {
            let _ = out_tx.push(sample);
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
            let stream = device
                .build_input_stream(
                    &cpal::StreamConfig {
                        channels,
                        sample_rate: cpal::SampleRate(config.sample_rate),
                        buffer_size,
                    },
                    move |data: &[f32], _| {
                        // Whole interleaved device frames; the assembler
                        // re-homes them at this device's offset.
                        for &sample in data {
                            let _ = tx.push(sample);
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

/// Build and start the output stream — the engine's clock. The callback
/// owns the engine and the assembler via `render`.
fn build_output(
    config: &StreamConfig,
    mut render: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<cpal::Stream, AudioError> {
    let host = cpal::default_host();
    let output_device = host
        .default_output_device()
        .ok_or_else(|| AudioError::Device("no default output device".into()))?;
    let stream = output_device
        .build_output_stream(
            &cpal::StreamConfig {
                channels: 2,
                sample_rate: cpal::SampleRate(config.sample_rate),
                buffer_size: cpal::BufferSize::Fixed(config.block_size as u32),
            },
            move |data: &mut [f32], _| render(data),
            |e| tracing::error!(%e, "output stream error"),
            None,
        )
        .map_err(|e| AudioError::Stream(format!("output: {e}")))?;
    stream
        .play()
        .map_err(|e| AudioError::Stream(format!("output play: {e}")))?;
    Ok(stream)
}
