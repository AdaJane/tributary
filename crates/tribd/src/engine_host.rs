//! The control task: single owner of the authoritative `MixerState`, the
//! engine command ring, and the current compile's parameter map. Every
//! mutation path (REST handlers, WS `set`) funnels here, so ordering is
//! total and the pure reducer is the only way state changes.
//!
//! Transport (record/play/seek) is imperative-shell state owned here, not
//! by the reducer: it is machinery, not mix document.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rtrb::{Producer, RingBuffer};
use tokio::sync::{mpsc, oneshot, watch};
use trib_core::{
    EqBand, FaderTarget, MeterKey, MixCommand, MixError, MixerState, ReconcileNeed, StateDelta,
    apply, db_to_linear,
};
use trib_dsp::{SmoothedParam, band_coefficients};
use trib_engine::{
    EngineCommand, ParamMap, PlaybackSet, PlaybackShared, PlaybackTrack, RECORD_RING_SECS,
    RecordSet, RecordTrack, compile,
};
use trib_project::{
    FeederHandle, PlaybackSource, Project, ProjectManifest, RecordFormat, TakeTrackInfo,
    TakeTrackSpec, TrackFile, TrackSink, create_project, list_takes, load_latest, open_and_prime,
    save_manifest, spawn_writer,
};

use crate::api::recording::RecordingSettingsDto;
use crate::api::ws::{
    Channel, LaneDto, LoopRegionDto, MonitorTarget, ServerMessage, TransportDto, TransportPhase,
    WsAck,
};
use crate::hub::Hub;
use crate::recording_prefs::RecordingPrefs;

/// Quiet time after the last edit before the manifest hits disk. Gestures
/// coalesce; a power cut costs at most this much mixing.
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// Playhead publish cadence — matches the meter pump.
const POSITION_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub enum TransportError {
    AlreadyRecording,
    NothingArmed,
    /// The transport is doing something that excludes the request.
    Busy(&'static str),
    /// PLAY with an empty shelf.
    NoTake,
    /// A lane index past the take's tracks.
    NoSuchLane,
    /// A loop region the take can't honor.
    BadLoop(&'static str),
    /// The take was cut at a different rate than the engine runs — no
    /// resampler in v1.
    SampleRateMismatch,
    /// A recording setting the daemon can't honor (unwritable destination,
    /// off-whitelist rate).
    BadSetting(String),
    Io(String),
}

pub enum ControlMsg {
    Apply {
        command: MixCommand,
        ack: Option<WsAck>,
        reply: oneshot::Sender<Result<StateDelta, MixError>>,
    },
    Snapshot {
        reply: oneshot::Sender<MixerState>,
    },
    RecordStart {
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    RecordStop {
        reply: oneshot::Sender<TransportDto>,
    },
    Play {
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    PlayStop {
        reply: oneshot::Sender<TransportDto>,
    },
    Seek {
        frames: u64,
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    /// Set or clear the loop region. Applying mid-play rebuilds the
    /// session (the one seek path).
    SetLoop {
        region: Option<LoopRegionDto>,
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    /// Route the playback mix: hardware out or the browser stream.
    SetMonitor {
        target: MonitorTarget,
        reply: oneshot::Sender<TransportDto>,
    },
    /// Solo/mute one playback lane. None = leave that flag alone.
    SetLaneGate {
        track: u32,
        solo: Option<bool>,
        mute: Option<bool>,
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    Transport {
        reply: oneshot::Sender<TransportDto>,
    },
    Autosave {
        generation: u64,
    },
    /// From the ticker: the engine drained the tape to the end.
    PlaybackFinished {
        generation: u64,
    },
    /// From the writer-join task: `take.toml` is on disk, the shelf moved.
    TakeFinalized,
    /// From the device orchestrator: input devices opened/closed, patches
    /// resolve differently now. Recompile (or stash while recording).
    InputSlotsChanged {
        slots: trib_engine::InputSlots,
    },
    RecordingSettings {
        reply: oneshot::Sender<RecordingSettingsDto>,
    },
    /// Update recording prefs. A destination change swaps the project root
    /// live: refused while recording, playback stops, and the next take
    /// lands on the new root. None = leave that setting alone.
    UpdateRecording {
        destination: Option<PathBuf>,
        format: Option<RecordFormat>,
        sample_rate: Option<u32>,
        reply: oneshot::Sender<Result<RecordingSettingsDto, TransportError>>,
    },
}

#[derive(Clone)]
pub struct ControlHandle {
    tx: mpsc::Sender<ControlMsg>,
}

impl ControlHandle {
    /// Apply one command. `ack` tags the resulting broadcast so the issuing
    /// client can reconcile its optimistic state.
    pub async fn apply(
        &self,
        command: MixCommand,
        ack: Option<WsAck>,
    ) -> Result<StateDelta, MixError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Apply {
                command,
                ack,
                reply,
            })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn snapshot(&self) -> MixerState {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Snapshot { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn record_start(&self) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::RecordStart { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn record_stop(&self) -> TransportDto {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::RecordStop { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn play(&self) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Play { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn play_stop(&self) -> TransportDto {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::PlayStop { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn seek(&self, frames: u64) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Seek { frames, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn set_monitor(&self, target: MonitorTarget) -> TransportDto {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::SetMonitor { target, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn set_loop(
        &self,
        region: Option<LoopRegionDto>,
    ) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::SetLoop { region, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn set_lane_gate(
        &self,
        track: u32,
        solo: Option<bool>,
        mute: Option<bool>,
    ) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::SetLaneGate {
                track,
                solo,
                mute,
                reply,
            })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn transport(&self) -> TransportDto {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Transport { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    /// The orchestrator thread's slot updates — blocking send, called
    /// off-runtime only.
    pub fn input_slots_changed_blocking(&self, slots: trib_engine::InputSlots) {
        let _ = self
            .tx
            .blocking_send(ControlMsg::InputSlotsChanged { slots });
    }

    pub async fn recording_settings(&self) -> RecordingSettingsDto {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::RecordingSettings { reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn update_recording(
        &self,
        destination: Option<PathBuf>,
        format: Option<RecordFormat>,
        sample_rate: Option<u32>,
    ) -> Result<RecordingSettingsDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::UpdateRecording {
                destination,
                format,
                sample_rate,
                reply,
            })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }
}

pub struct EngineConfig {
    pub sample_rate: u32,
    pub block_size: usize,
    /// Bumped per playback session so monitor-stream clients flush their
    /// buffers instead of splicing takes together.
    pub monitor_generation: Arc<AtomicU32>,
}

/// The recording configuration the control task owns: user prefs
/// (persisted to recording.toml), the boot defaults they fall back to, and
/// the root recording actually lands on right now.
pub struct RecordingHost {
    pub prefs: RecordingPrefs,
    pub prefs_path: PathBuf,
    pub default_root: PathBuf,
    pub default_sample_rate: u32,
    /// Where takes land right now — differs from `prefs.destination` after
    /// a boot-time fallback (configured drive unplugged).
    pub active_root: PathBuf,
    /// Publishes the open project to the API (take listings, peaks).
    pub project_watch: watch::Sender<Arc<Project>>,
}

impl RecordingHost {
    fn dto(&self, engine_rate: u32, project: &Project) -> RecordingSettingsDto {
        let configured_sample_rate = self.prefs.sample_rate.unwrap_or(self.default_sample_rate);
        RecordingSettingsDto {
            destination: self.active_root.display().to_string(),
            configured_destination: self
                .prefs
                .destination
                .as_ref()
                .map(|p| p.display().to_string()),
            default_destination: self.default_root.display().to_string(),
            project_name: project.name.clone(),
            format: self.prefs.format,
            active_sample_rate: engine_rate,
            configured_sample_rate,
            restart_required: configured_sample_rate != engine_rate,
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "startup wiring, called once")]
pub fn spawn(
    hub: Hub,
    cmd_tx: Producer<EngineCommand>,
    initial: MixerState,
    initial_params: ParamMap,
    config: EngineConfig,
    meter_keys: watch::Sender<Vec<MeterKey>>,
    project: Project,
    project_created_at: u64,
    recording: RecordingHost,
    devices: Option<crate::device_host::DeviceHandle>,
) -> ControlHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(control_loop(
        hub,
        cmd_tx,
        initial,
        initial_params,
        config,
        meter_keys,
        project,
        project_created_at,
        recording,
        devices,
        tx.clone(),
        rx,
    ));
    ControlHandle { tx }
}

struct RecordingTake {
    take: u32,
    started_at_unix: u64,
    /// Joined (off-loop) after stop; its exit means `take.toml` is real.
    writer: std::thread::JoinHandle<()>,
}

struct PlaybackSession {
    /// Guards stale `PlaybackFinished` from a torn-down ticker.
    generation: u64,
    start_frame: u64,
    /// The loop the session was built with — position mapping must match
    /// what the feeder is actually wrapping.
    loop_region: Option<LoopRegionDto>,
    shared: Arc<PlaybackShared>,
    ticker: tokio::task::JoinHandle<()>,
    /// Dropping the session stops the feeder thread.
    _feeder: FeederHandle,
}

/// Map monotone frames-played onto the take timeline, folding loop passes.
/// The audio thread stays loop-ignorant; this is the one mapping.
fn timeline_position(start_frame: u64, loop_region: Option<LoopRegionDto>, played: u64) -> u64 {
    let pos = start_frame + played;
    match loop_region {
        Some(l) if l.end_frames > l.start_frames && pos >= l.end_frames => {
            let span = l.end_frames - l.start_frames;
            l.start_frames + (pos - l.end_frames) % span
        }
        _ => pos,
    }
}

enum TransportState {
    Stopped,
    Playing(PlaybackSession),
    Recording(RecordingTake),
}

/// The latest take, cached so every transport reply doesn't re-read disk.
/// Refreshed at boot and on `TakeFinalized`.
#[derive(Clone)]
struct LatestTake {
    take: u32,
    sample_rate: u32,
    format: trib_project::RecordFormat,
    total_frames: u64,
    tracks: Vec<TakeTrackInfo>,
}

fn load_latest_take(project: &Project) -> Option<LatestTake> {
    let info = list_takes(project).into_iter().next()?;
    Some(LatestTake {
        take: info.take,
        sample_rate: info.sample_rate,
        format: info.format,
        total_frames: info.tracks.iter().map(|t| t.frames).max().unwrap_or(0),
        tracks: info.tracks,
    })
}

/// All transport state, bundled so the helpers stay readable.
struct TransportCtl {
    state: TransportState,
    latest: Option<LatestTake>,
    /// Playback gates, index-aligned with the latest take's tracks.
    lanes: Vec<LaneDto>,
    loop_region: Option<LoopRegionDto>,
    /// Where PLAY resumes; parked by stop, moved by seek, 0 after the end.
    stopped_position: u64,
    generation: u64,
    engine_rate: u32,
    monitor: MonitorTarget,
    monitor_generation: Arc<AtomicU32>,
}

impl TransportCtl {
    fn new(project: &Project, engine_rate: u32, monitor_generation: Arc<AtomicU32>) -> Self {
        let latest = load_latest_take(project);
        let lanes = vec![LaneDto::default(); latest.as_ref().map_or(0, |l| l.tracks.len())];
        TransportCtl {
            state: TransportState::Stopped,
            latest,
            lanes,
            loop_region: None,
            stopped_position: 0,
            generation: 0,
            engine_rate,
            monitor: MonitorTarget::Hardware,
            monitor_generation,
        }
    }

    /// A new take resets the review posture: gates open, loop gone, tape
    /// rewound.
    fn refresh_latest(&mut self, project: &Project) {
        self.latest = load_latest_take(project);
        self.lanes = vec![LaneDto::default(); self.latest.as_ref().map_or(0, |l| l.tracks.len())];
        self.loop_region = None;
        self.stopped_position = 0;
    }

    fn recording(&self) -> bool {
        matches!(self.state, TransportState::Recording(_))
    }

    fn dto(&self) -> TransportDto {
        let (phase, take, started_at_unix, position_frames) = match &self.state {
            TransportState::Stopped => (
                TransportPhase::Stopped,
                self.latest.as_ref().map(|l| l.take),
                None,
                self.stopped_position,
            ),
            TransportState::Playing(s) => (
                TransportPhase::Playing,
                self.latest.as_ref().map(|l| l.take),
                None,
                timeline_position(
                    s.start_frame,
                    s.loop_region,
                    s.shared.position.load(Ordering::Relaxed),
                ),
            ),
            TransportState::Recording(r) => (
                TransportPhase::Recording,
                Some(r.take),
                Some(r.started_at_unix),
                0,
            ),
        };
        TransportDto {
            state: phase,
            take,
            started_at_unix,
            position_frames,
            sample_rate: self
                .latest
                .as_ref()
                .map_or(self.engine_rate, |l| l.sample_rate),
            total_frames: self.latest.as_ref().map_or(0, |l| l.total_frames),
            loop_region: self.loop_region,
            monitor: self.monitor,
            lanes: self.lanes.clone(),
        }
    }
}

/// The device identities the console references — what the orchestrator
/// must keep open. `None` = the system default input.
fn wanted_devices(state: &MixerState) -> std::collections::BTreeSet<Option<String>> {
    state
        .strips
        .iter()
        .filter_map(|s| s.input.as_ref())
        .map(|a| a.device.clone())
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_secs()
}

fn file_slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

fn publish_transport(hub: &Hub, ctl: &TransportCtl) {
    hub.publish(
        Channel::Transport,
        &ServerMessage::Transport { state: ctl.dto() },
    );
}

#[expect(clippy::too_many_arguments, reason = "startup wiring, called once")]
async fn control_loop(
    hub: Hub,
    mut cmd_tx: Producer<EngineCommand>,
    mut state: MixerState,
    mut params: ParamMap,
    config: EngineConfig,
    meter_keys: watch::Sender<Vec<MeterKey>>,
    mut project: Project,
    mut project_created_at: u64,
    mut recording: RecordingHost,
    devices: Option<crate::device_host::DeviceHandle>,
    self_tx: mpsc::Sender<ControlMsg>,
    mut rx: mpsc::Receiver<ControlMsg>,
) {
    let mut ctl = TransportCtl::new(
        &project,
        config.sample_rate,
        config.monitor_generation.clone(),
    );
    // Patches resolve against what the device orchestrator has actually
    // opened — empty until its first slot map arrives.
    let mut input_slots = trib_engine::InputSlots::default();
    // Slot changes landing mid-take wait: record taps are bound to the
    // running graph, so the swap happens at RecordStop.
    let mut pending_slots: Option<trib_engine::InputSlots> = None;
    // Tell the orchestrator what the loaded project wants, and again on
    // every change.
    let mut last_wanted = wanted_devices(&state);
    if let Some(devices) = &devices {
        devices.wanted_changed(last_wanted.clone());
    }
    let mut save_generation: u64 = 0;
    while let Some(msg) = rx.recv().await {
        match msg {
            ControlMsg::Apply {
                command,
                ack,
                reply,
            } => match apply(&state, command) {
                Ok((next, delta, need)) => {
                    // The console layout is frozen while tape rolls: track
                    // taps are bound to the running graph's strip order.
                    if need == ReconcileNeed::Topology && ctl.recording() {
                        let _ = reply.send(Err(MixError::Unsupported(
                            "stop recording before changing the console layout",
                        )));
                        continue;
                    }
                    state = next;
                    let wanted = wanted_devices(&state);
                    if wanted != last_wanted {
                        last_wanted = wanted.clone();
                        if let Some(devices) = &devices {
                            devices.wanted_changed(wanted);
                        }
                    }
                    match need {
                        ReconcileNeed::ParamOnly => {
                            for engine_cmd in param_commands(&params, &delta, config.sample_rate) {
                                push(&mut cmd_tx, engine_cmd);
                            }
                            hub.publish(
                                Channel::Mixer,
                                &ServerMessage::StateChanged {
                                    delta: delta.clone(),
                                    ack,
                                },
                            );
                        }
                        // Structural change: recompile, swap, and resync
                        // every client with the whole document.
                        ReconcileNeed::Topology => {
                            let compiled = compile(
                                &state,
                                config.sample_rate,
                                config.block_size,
                                &input_slots,
                            );
                            params = compiled.params;
                            let _ = meter_keys.send(compiled.meter_keys);
                            push(
                                &mut cmd_tx,
                                EngineCommand::SwapGraph {
                                    graph: compiled.graph,
                                },
                            );
                            hub.publish(
                                Channel::Mixer,
                                &ServerMessage::MixerSnapshot {
                                    state: state.clone(),
                                },
                            );
                        }
                    }
                    // Debounced autosave: only the newest generation lands.
                    save_generation += 1;
                    let generation = save_generation;
                    let tx = self_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(AUTOSAVE_DEBOUNCE).await;
                        let _ = tx.send(ControlMsg::Autosave { generation }).await;
                    });
                    let _ = reply.send(Ok(delta));
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            },
            ControlMsg::Snapshot { reply } => {
                let _ = reply.send(state.clone());
            }
            ControlMsg::RecordStart { reply } => {
                // Tape wins: review mode never blocks a take. Stop playback
                // and proceed.
                if matches!(ctl.state, TransportState::Playing(_)) {
                    stop_playback(&mut ctl, &mut cmd_tx, &hub);
                }
                let result = start_recording(
                    &state,
                    &project,
                    &config,
                    recording.prefs.format,
                    &mut cmd_tx,
                    &mut ctl,
                    &hub,
                );
                if result.is_ok() {
                    publish_transport(&hub, &ctl);
                }
                let _ = reply.send(result);
            }
            ControlMsg::RecordStop { reply } => {
                if ctl.recording() {
                    let TransportState::Recording(rec) =
                        std::mem::replace(&mut ctl.state, TransportState::Stopped)
                    else {
                        unreachable!()
                    };
                    push(&mut cmd_tx, EngineCommand::StopRecord);
                    // The writer exits once the retired record set drops its
                    // producers; its exit means take.toml is on disk.
                    let tx = self_tx.clone();
                    tokio::spawn(async move {
                        let _ = tokio::task::spawn_blocking(move || rec.writer.join()).await;
                        let _ = tx.send(ControlMsg::TakeFinalized).await;
                    });
                    // Device changes that arrived mid-take land now.
                    if let Some(slots) = pending_slots.take() {
                        input_slots = slots;
                        let compiled =
                            compile(&state, config.sample_rate, config.block_size, &input_slots);
                        params = compiled.params;
                        let _ = meter_keys.send(compiled.meter_keys);
                        push(
                            &mut cmd_tx,
                            EngineCommand::SwapGraph {
                                graph: compiled.graph,
                            },
                        );
                    }
                    publish_transport(&hub, &ctl);
                }
                let _ = reply.send(ctl.dto());
            }
            ControlMsg::Play { reply } => {
                let result = start_playback(&mut ctl, &project, &mut cmd_tx, &hub, &self_tx).await;
                let _ = reply.send(result);
            }
            ControlMsg::PlayStop { reply } => {
                if matches!(ctl.state, TransportState::Playing(_)) {
                    stop_playback(&mut ctl, &mut cmd_tx, &hub);
                }
                let _ = reply.send(ctl.dto());
            }
            ControlMsg::Seek { frames, reply } => {
                let result = match ctl.state {
                    TransportState::Recording(_) => {
                        Err(TransportError::Busy("no seeking while recording"))
                    }
                    TransportState::Stopped => {
                        let total = ctl.latest.as_ref().map_or(0, |l| l.total_frames);
                        ctl.stopped_position = frames.min(total);
                        publish_transport(&hub, &ctl);
                        Ok(ctl.dto())
                    }
                    // Seek = teardown + rebuild at the target: no stale
                    // samples by construction.
                    TransportState::Playing(_) => {
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                        let total = ctl.latest.as_ref().map_or(0, |l| l.total_frames);
                        ctl.stopped_position = frames.min(total);
                        start_playback(&mut ctl, &project, &mut cmd_tx, &hub, &self_tx).await
                    }
                };
                let _ = reply.send(result);
            }
            ControlMsg::SetLoop { region, reply } => {
                let result = 'set: {
                    if ctl.recording() {
                        break 'set Err(TransportError::Busy("no loop edits while recording"));
                    }
                    let Some(latest) = &ctl.latest else {
                        break 'set Err(TransportError::NoTake);
                    };
                    if let Some(l) = region {
                        // A tenth of a second is the floor a reopen-per-pass
                        // feeder can wrap comfortably.
                        let min_frames = u64::from(latest.sample_rate / 10);
                        if l.end_frames <= l.start_frames {
                            break 'set Err(TransportError::BadLoop("loop ends before it starts"));
                        }
                        if l.end_frames - l.start_frames < min_frames {
                            break 'set Err(TransportError::BadLoop("loop shorter than 0.1s"));
                        }
                        if l.end_frames > latest.total_frames {
                            break 'set Err(TransportError::BadLoop("loop runs past the take"));
                        }
                    }
                    ctl.loop_region = region;
                    if matches!(ctl.state, TransportState::Playing(_)) {
                        // The one seek path: rebuild the session under the
                        // new region from the mapped current position.
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                        start_playback(&mut ctl, &project, &mut cmd_tx, &hub, &self_tx).await
                    } else {
                        publish_transport(&hub, &ctl);
                        Ok(ctl.dto())
                    }
                };
                let _ = reply.send(result);
            }
            ControlMsg::SetMonitor { target, reply } => {
                ctl.monitor = target;
                push(
                    &mut cmd_tx,
                    EngineCommand::SetMonitorTarget {
                        target: match target {
                            MonitorTarget::Hardware => trib_engine::MonitorTarget::Hardware,
                            MonitorTarget::Stream => trib_engine::MonitorTarget::Stream,
                        },
                    },
                );
                publish_transport(&hub, &ctl);
                let _ = reply.send(ctl.dto());
            }
            ControlMsg::SetLaneGate {
                track,
                solo,
                mute,
                reply,
            } => {
                let result = if (track as usize) >= ctl.lanes.len() {
                    Err(TransportError::NoSuchLane)
                } else {
                    let lane = &mut ctl.lanes[track as usize];
                    if let Some(on) = solo {
                        lane.solo = on;
                    }
                    if let Some(on) = mute {
                        lane.mute = on;
                    }
                    // Live gates ride the command ring; while stopped the
                    // engine has no session and the next build reapplies
                    // them from `ctl.lanes`.
                    let track = track as u16;
                    if let Some(on) = solo {
                        push(&mut cmd_tx, EngineCommand::SetPlaybackSolo { track, on });
                    }
                    if let Some(on) = mute {
                        push(&mut cmd_tx, EngineCommand::SetPlaybackMute { track, on });
                    }
                    publish_transport(&hub, &ctl);
                    Ok(ctl.dto())
                };
                let _ = reply.send(result);
            }
            ControlMsg::PlaybackFinished { generation } => {
                let current = match &ctl.state {
                    TransportState::Playing(s) => s.generation == generation,
                    _ => false,
                };
                if current {
                    // The engine already retired the set; rewind the tape.
                    ctl.state = TransportState::Stopped;
                    ctl.stopped_position = 0;
                    publish_transport(&hub, &ctl);
                }
            }
            ControlMsg::TakeFinalized => {
                ctl.refresh_latest(&project);
                publish_transport(&hub, &ctl);
            }
            ControlMsg::InputSlotsChanged { slots } => {
                if ctl.recording() {
                    // Applied at RecordStop: the running graph's record
                    // taps must not be swapped out from under the take.
                    pending_slots = Some(slots);
                } else {
                    input_slots = slots;
                    let compiled =
                        compile(&state, config.sample_rate, config.block_size, &input_slots);
                    params = compiled.params;
                    let _ = meter_keys.send(compiled.meter_keys);
                    push(
                        &mut cmd_tx,
                        EngineCommand::SwapGraph {
                            graph: compiled.graph,
                        },
                    );
                    // No MixerSnapshot: the document didn't change, only
                    // where its patches physically land.
                }
            }
            ControlMsg::Transport { reply } => {
                let _ = reply.send(ctl.dto());
            }
            ControlMsg::Autosave { generation } => {
                if generation == save_generation
                    && let Err(e) = save_manifest(&project, &state, project_created_at)
                {
                    tracing::error!(%e, "autosave failed");
                }
            }
            ControlMsg::RecordingSettings { reply } => {
                let _ = reply.send(recording.dto(ctl.engine_rate, &project));
            }
            ControlMsg::UpdateRecording {
                destination,
                format,
                sample_rate,
                reply,
            } => {
                let result = 'update: {
                    // Validate everything before applying anything — the
                    // PUT is atomic.
                    if let Some(rate) = sample_rate
                        && !crate::settings::SAMPLE_RATES.contains(&rate)
                    {
                        break 'update Err(TransportError::BadSetting(format!(
                            "sample_rate must be one of {:?}",
                            crate::settings::SAMPLE_RATES
                        )));
                    }
                    if destination.is_some() && ctl.recording() {
                        break 'update Err(TransportError::Busy(
                            "stop recording before changing the destination",
                        ));
                    }
                    if let Some(new_root) = destination {
                        if matches!(ctl.state, TransportState::Playing(_)) {
                            stop_playback(&mut ctl, &mut cmd_tx, &hub);
                        }
                        let seed = state.clone();
                        let probe_root = new_root.clone();
                        let opened = tokio::task::spawn_blocking(move || {
                            open_destination(&probe_root, &seed)
                        })
                        .await
                        .expect("destination task is never cancelled");
                        let (new_project, manifest, adopted) = match opened {
                            Ok(opened) => opened,
                            Err(e) => break 'update Err(e),
                        };
                        if adopted {
                            // The boot path, mid-flight: the destination's
                            // own console takes over the desk.
                            state = manifest.mixer;
                            let wanted = wanted_devices(&state);
                            if wanted != last_wanted {
                                last_wanted = wanted.clone();
                                if let Some(devices) = &devices {
                                    devices.wanted_changed(wanted);
                                }
                            }
                            let compiled = compile(
                                &state,
                                config.sample_rate,
                                config.block_size,
                                &input_slots,
                            );
                            params = compiled.params;
                            let _ = meter_keys.send(compiled.meter_keys);
                            push(
                                &mut cmd_tx,
                                EngineCommand::SwapGraph {
                                    graph: compiled.graph,
                                },
                            );
                            hub.publish(
                                Channel::Mixer,
                                &ServerMessage::MixerSnapshot {
                                    state: state.clone(),
                                },
                            );
                        }
                        // In-flight autosaves aimed at the old dir are
                        // stale now.
                        save_generation += 1;
                        project = new_project;
                        project_created_at = manifest.created_at_unix;
                        recording.prefs.destination = Some(new_root.clone());
                        recording.active_root = new_root;
                        let _ = recording.project_watch.send(Arc::new(project.clone()));
                        ctl.refresh_latest(&project);
                        publish_transport(&hub, &ctl);
                    }
                    if let Some(format) = format {
                        recording.prefs.format = format;
                    }
                    if let Some(rate) = sample_rate {
                        recording.prefs.sample_rate = Some(rate);
                    }
                    if let Err(e) = recording.prefs.save(&recording.prefs_path) {
                        tracing::error!(%e, "recording prefs save failed");
                    }
                    Ok(recording.dto(ctl.engine_rate, &project))
                };
                let _ = reply.send(result);
            }
        }
    }
}

/// Blocking: ensure the destination exists and is writable, then adopt its
/// newest project — or tear off fresh tape seeded with the current console.
fn open_destination(
    root: &Path,
    seed: &MixerState,
) -> Result<(Project, ProjectManifest, bool), TransportError> {
    std::fs::create_dir_all(root)
        .map_err(|e| TransportError::BadSetting(format!("{}: {e}", root.display())))?;
    let probe = root.join(".tribd-probe");
    std::fs::write(&probe, b"tributary")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|e| {
            TransportError::BadSetting(format!("{} is not writable: {e}", root.display()))
        })?;
    if let Some((project, manifest)) = load_latest(root) {
        return Ok((project, manifest, true));
    }
    let project =
        create_project(root, "Session", seed).map_err(|e| TransportError::Io(e.to_string()))?;
    let manifest = load_latest(root).expect("just-created project loads").1;
    Ok((project, manifest, false))
}

/// Tear down the current session (the ticker, the feeder, the engine's
/// set), park the playhead where it stopped, and tell everyone.
fn stop_playback(ctl: &mut TransportCtl, cmd_tx: &mut Producer<EngineCommand>, hub: &Hub) {
    let TransportState::Playing(session) =
        std::mem::replace(&mut ctl.state, TransportState::Stopped)
    else {
        return;
    };
    push(cmd_tx, EngineCommand::StopPlayback);
    session.ticker.abort();
    ctl.stopped_position = timeline_position(
        session.start_frame,
        session.loop_region,
        session.shared.position.load(Ordering::Relaxed),
    );
    drop(session); // the feeder handle stops its thread
    publish_transport(hub, ctl);
}

/// Open the latest take, prime the rings off the async runtime, and roll.
async fn start_playback(
    ctl: &mut TransportCtl,
    project: &Project,
    cmd_tx: &mut Producer<EngineCommand>,
    hub: &Hub,
    self_tx: &mpsc::Sender<ControlMsg>,
) -> Result<TransportDto, TransportError> {
    match ctl.state {
        TransportState::Recording(_) => {
            return Err(TransportError::Busy("stop recording before playback"));
        }
        TransportState::Playing(_) => return Ok(ctl.dto()), // idempotent
        TransportState::Stopped => {}
    }
    let latest = ctl.latest.clone().ok_or(TransportError::NoTake)?;
    if latest.sample_rate != ctl.engine_rate {
        return Err(TransportError::SampleRateMismatch);
    }
    // Playing past the end means "from the top"; a parked playhead past
    // the loop means "from the loop start".
    let start_frame = {
        let base = if ctl.stopped_position >= latest.total_frames {
            0
        } else {
            ctl.stopped_position
        };
        match ctl.loop_region {
            Some(l) if base >= l.end_frames => l.start_frames,
            _ => base,
        }
    };

    let source = PlaybackSource {
        take_dir: project.takes_dir().join(format!("take-{:03}", latest.take)),
        tracks: latest
            .tracks
            .iter()
            .map(|t| TrackFile {
                file: t.file.clone(),
                channels: t.channels,
            })
            .collect(),
        sample_rate: latest.sample_rate,
        format: latest.format,
        total_frames: latest.total_frames,
        start_frame,
        loop_region: ctl.loop_region.map(|l| (l.start_frames, l.end_frames)),
    };
    let (consumers, feeder) = tokio::task::spawn_blocking(move || open_and_prime(&source))
        .await
        .expect("prime task is never cancelled")
        .map_err(|e| TransportError::Io(e.to_string()))?;

    let tracks: Vec<PlaybackTrack> = consumers
        .into_iter()
        .zip(latest.tracks.iter().zip(&ctl.lanes))
        .map(|(rx, (info, gate))| PlaybackTrack {
            rx,
            channels: info.channels,
            // Only the master track is stereo in v1.
            is_master: info.channels == 2,
            solo: gate.solo,
            mute: gate.mute,
            gate: SmoothedParam::new(0.0, ctl.engine_rate),
        })
        .collect();
    let has_master = tracks.iter().any(|t| t.is_master);
    let set = PlaybackSet::new(tracks, has_master);
    let shared = set.shared.clone();
    push(cmd_tx, EngineCommand::StartPlayback { set: Box::new(set) });

    ctl.generation += 1;
    // Stream clients flush their buffers on a generation change, so a seek
    // never splices two sessions together.
    ctl.monitor_generation
        .store(ctl.generation as u32, Ordering::Relaxed);
    let ticker = spawn_ticker(
        hub.clone(),
        self_tx.clone(),
        shared.clone(),
        latest.take,
        start_frame,
        ctl.loop_region,
        ctl.generation,
    );
    ctl.state = TransportState::Playing(PlaybackSession {
        generation: ctl.generation,
        start_frame,
        loop_region: ctl.loop_region,
        shared,
        ticker,
        _feeder: feeder,
    });
    let dto = ctl.dto();
    hub.publish(
        Channel::Transport,
        &ServerMessage::Transport { state: dto.clone() },
    );
    Ok(dto)
}

/// Publish the playhead from real frames at the pump rate; hand the finish
/// back to the control loop (generation-guarded against stale sessions).
fn spawn_ticker(
    hub: Hub,
    self_tx: mpsc::Sender<ControlMsg>,
    shared: Arc<PlaybackShared>,
    take: u32,
    start_frame: u64,
    loop_region: Option<LoopRegionDto>,
    generation: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POSITION_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if shared.finished.load(Ordering::Relaxed) {
                let _ = self_tx
                    .send(ControlMsg::PlaybackFinished { generation })
                    .await;
                return;
            }
            hub.publish(
                Channel::Transport,
                &ServerMessage::PlaybackPosition {
                    take,
                    frames: timeline_position(
                        start_frame,
                        loop_region,
                        shared.position.load(Ordering::Relaxed),
                    ),
                },
            );
        }
    })
}

/// Build the take: rings + writer thread + the engine's record set. The
/// strip→track mapping is bound to the CURRENT compile's strip order,
/// which the topology guard keeps frozen while recording.
fn start_recording(
    state: &MixerState,
    project: &Project,
    config: &EngineConfig,
    format: RecordFormat,
    cmd_tx: &mut Producer<EngineCommand>,
    ctl: &mut TransportCtl,
    hub: &Hub,
) -> Result<TransportDto, TransportError> {
    if ctl.recording() {
        return Err(TransportError::AlreadyRecording);
    }
    let armed: Vec<usize> = state
        .strips
        .iter()
        .enumerate()
        .filter(|(_, s)| s.record_arm)
        .map(|(i, _)| i)
        .collect();
    if armed.is_empty() && !state.master.record_arm {
        return Err(TransportError::NothingArmed);
    }

    let take = project.next_take();
    let take_dir = project.takes_dir().join(format!("take-{take:03}"));
    let started_at_unix = now_unix();
    let ring_capacity = config.sample_rate as usize * RECORD_RING_SECS;

    let mut strip_tracks: Vec<Option<u16>> = vec![None; state.strips.len()];
    let mut tracks = Vec::new();
    let mut sinks = Vec::new();
    for strip_ix in armed {
        let strip = &state.strips[strip_ix];
        let (tx, rx) = RingBuffer::new(ring_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        strip_tracks[strip_ix] = Some(tracks.len() as u16);
        tracks.push(RecordTrack {
            tx,
            dropped: dropped.clone(),
        });
        sinks.push(TrackSink {
            spec: TakeTrackSpec {
                file: format!(
                    "ch{:02}-{}.{}",
                    strip.id.0 + 1,
                    file_slug(&strip.name),
                    format.extension()
                ),
                channels: 1,
                strip_id: Some(strip.id.0),
            },
            rx,
            dropped,
        });
    }
    let master_track = state.master.record_arm.then(|| {
        let (tx, rx) = RingBuffer::new(ring_capacity * 2);
        let dropped = Arc::new(AtomicU64::new(0));
        let ix = tracks.len() as u16;
        tracks.push(RecordTrack {
            tx,
            dropped: dropped.clone(),
        });
        sinks.push(TrackSink {
            spec: TakeTrackSpec {
                file: format!("master.{}", format.extension()),
                channels: 2,
                strip_id: None,
            },
            rx,
            dropped,
        });
        ix
    });

    // The writer taps freshly closed peak bins into a channel; a forwarder
    // fans them onto the Waveform WS channel — lanes grow in real time.
    let (peaks_tx, mut peaks_rx) = mpsc::unbounded_channel::<trib_project::PeakBatch>();
    let tap: trib_project::PeaksTap = Box::new(move |batch| {
        let _ = peaks_tx.send(batch);
    });
    let forward_hub = hub.clone();
    tokio::spawn(async move {
        while let Some(batch) = peaks_rx.recv().await {
            forward_hub.publish(
                Channel::Waveform,
                &ServerMessage::WaveformBins {
                    take,
                    track: batch.track as u32,
                    start_bin: batch.start_bin,
                    samples_per_bin: trib_project::PEAK_SAMPLES_PER_BIN,
                    bins: batch.bins,
                },
            );
        }
    });
    let writer = spawn_writer(
        take_dir,
        config.sample_rate,
        started_at_unix,
        format,
        sinks,
        Some(tap),
    )
    .map_err(|e| TransportError::Io(e.to_string()))?;
    push(
        cmd_tx,
        EngineCommand::StartRecord {
            set: Box::new(RecordSet {
                strip_tracks,
                master_track,
                tracks,
            }),
        },
    );
    ctl.state = TransportState::Recording(RecordingTake {
        take,
        started_at_unix,
        writer,
    });
    Ok(ctl.dto())
}

/// Never wait on the audio thread. A full ring means it stalled; state
/// stays authoritative and the next successful push re-converges (targets
/// are absolute, not increments).
fn push(cmd_tx: &mut Producer<EngineCommand>, command: EngineCommand) {
    if cmd_tx.push(command).is_err() {
        tracing::error!("engine command ring full; dropping engine write");
    }
}

/// Map a ParamOnly delta onto the current compile's dispatch indices.
/// Misses are normal for targets the DSP doesn't wire yet (bus faders
/// before M4) and for renames.
fn param_commands(params: &ParamMap, delta: &StateDelta, sample_rate: u32) -> Vec<EngineCommand> {
    match delta {
        StateDelta::Fader { target, level_db } => {
            let ix = match target {
                FaderTarget::Strip { id } => params.fader.get(id).copied(),
                FaderTarget::Master => params.master_fader,
                FaderTarget::Bus { id } => params.bus_fader.get(id).copied(),
            };
            ix.map(|param| EngineCommand::SetParam {
                param,
                target: db_to_linear(*level_db),
            })
            .into_iter()
            .collect()
        }
        StateDelta::Gain { strip, gain_db } => params
            .gain
            .get(strip)
            .map(|&param| EngineCommand::SetParam {
                param,
                target: db_to_linear(*gain_db),
            })
            .into_iter()
            .collect(),
        StateDelta::Pan { strip, pan } => params
            .pan
            .get(strip)
            .map(|&param| EngineCommand::SetParam {
                param,
                target: *pan,
            })
            .into_iter()
            .collect(),
        StateDelta::Mute { target, mute } => {
            let ix = match target {
                FaderTarget::Strip { id } => params.mute.get(id).copied(),
                FaderTarget::Bus { id } => params.bus_mute.get(id).copied(),
                FaderTarget::Master => None,
            };
            ix.map(|param| EngineCommand::SetParam {
                param,
                target: if *mute { 0.0 } else { 1.0 },
            })
            .into_iter()
            .collect()
        }
        StateDelta::Pfl { target, on } => {
            let ix = match target {
                FaderTarget::Strip { id } => params.pfl.get(id).copied(),
                FaderTarget::Bus { id } => params.bus_pfl.get(id).copied(),
                FaderTarget::Master => None,
            };
            ix.map(|flag| EngineCommand::SetFlag { flag, on: *on })
                .into_iter()
                .collect()
        }
        StateDelta::EqEnabled { strip, enabled } => params
            .eq_enabled
            .get(strip)
            .map(|&flag| EngineCommand::SetFlag { flag, on: *enabled })
            .into_iter()
            .collect(),
        StateDelta::EqBand {
            strip,
            band,
            freq_hz,
            gain_db,
            q,
        } => params
            .eq_band
            .get(&(*strip, *band))
            .map(|&eq| EngineCommand::SetEqCoeffs {
                eq,
                coeffs: band_coefficients(
                    &EqBand {
                        kind: *band,
                        freq_hz: *freq_hz,
                        gain_db: *gain_db,
                        q: *q,
                    },
                    sample_rate,
                ),
            })
            .into_iter()
            .collect(),
        StateDelta::Send {
            strip,
            dest,
            level_db,
            tap,
        } => {
            let mut commands = Vec::new();
            if let Some(&param) = params.send.get(&(*strip, *dest)) {
                commands.push(EngineCommand::SetParam {
                    param,
                    target: db_to_linear(*level_db),
                });
            }
            if let Some(&flag) = params.send_pre.get(&(*strip, *dest)) {
                commands.push(EngineCommand::SetFlag {
                    flag,
                    on: *tap == trib_core::SendTap::PreFader,
                });
            }
            commands
        }
        StateDelta::FxParams { fx, params: p } => match *p {
            trib_core::FxParams::Reverb { room_size, damping } => [
                params
                    .fx_room
                    .get(fx)
                    .map(|&param| EngineCommand::SetParam {
                        param,
                        target: room_size,
                    }),
                params
                    .fx_damp
                    .get(fx)
                    .map(|&param| EngineCommand::SetParam {
                        param,
                        target: damping,
                    }),
            ]
            .into_iter()
            .flatten()
            .collect(),
            trib_core::FxParams::Delay { time_ms, feedback } => [
                params
                    .fx_time
                    .get(fx)
                    .map(|&param| EngineCommand::SetParam {
                        param,
                        target: time_ms,
                    }),
                params
                    .fx_feedback
                    .get(fx)
                    .map(|&param| EngineCommand::SetParam {
                        param,
                        target: feedback,
                    }),
            ]
            .into_iter()
            .flatten()
            .collect(),
        },
        StateDelta::FxReturn { fx, level_db } => params
            .fx_return
            .get(fx)
            .map(|&param| EngineCommand::SetParam {
                param,
                target: db_to_linear(*level_db),
            })
            .into_iter()
            .collect(),
        StateDelta::Renamed { .. } => vec![],
        // Arm flags only matter at the next record start.
        StateDelta::RecordArm { .. } | StateDelta::RecordArmAll { .. } => vec![],
        // Topology deltas never reach here.
        StateDelta::Input { .. }
        | StateDelta::StripAdded { .. }
        | StateDelta::StripRemoved { .. }
        | StateDelta::Route { .. }
        | StateDelta::BusAdded { .. }
        | StateDelta::BusRemoved { .. } => {
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use trib_core::{StripId, StripState};

    use super::*;

    fn initial() -> (MixerState, ParamMap, Vec<MeterKey>) {
        let state = MixerState {
            strips: vec![StripState::new(StripId(0), "Ch 1".into())],
            ..MixerState::default()
        };
        let compiled = compile(
            &state,
            48_000,
            256,
            &trib_engine::InputSlots::single_default(64),
        );
        (state, compiled.params, compiled.meter_keys)
    }

    fn config() -> EngineConfig {
        EngineConfig {
            sample_rate: 48_000,
            block_size: 256,
            monitor_generation: Arc::default(),
        }
    }

    /// spawn() with a throwaway project. The TempDir must outlive the
    /// control task, so it rides along in the return.
    fn spawn_test(
        hub: Hub,
        cmd_tx: Producer<EngineCommand>,
        state: MixerState,
        params: ParamMap,
        keys_tx: watch::Sender<Vec<MeterKey>>,
    ) -> (ControlHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);
        (control, dir)
    }

    /// A RecordingHost over a throwaway prefs file in the project dir's
    /// parent (the "config sibling" role doesn't matter under test).
    fn recording_host(project: &Project) -> (RecordingHost, watch::Receiver<Arc<Project>>) {
        let root = project
            .dir
            .parent()
            .expect("project has a parent")
            .to_path_buf();
        let (project_watch, watch_rx) = watch::channel(Arc::new(project.clone()));
        let host = RecordingHost {
            prefs: RecordingPrefs::default(),
            prefs_path: root.join("recording.toml"),
            default_root: root.clone(),
            default_sample_rate: 48_000,
            active_root: root,
            project_watch,
        };
        (host, watch_rx)
    }

    fn spawn_test_in(
        hub: Hub,
        cmd_tx: Producer<EngineCommand>,
        state: MixerState,
        params: ParamMap,
        keys_tx: watch::Sender<Vec<MeterKey>>,
        project: Project,
    ) -> ControlHandle {
        spawn_test_watched(hub, cmd_tx, state, params, keys_tx, project).0
    }

    fn spawn_test_watched(
        hub: Hub,
        cmd_tx: Producer<EngineCommand>,
        state: MixerState,
        params: ParamMap,
        keys_tx: watch::Sender<Vec<MeterKey>>,
        project: Project,
    ) -> (ControlHandle, watch::Receiver<Arc<Project>>) {
        let (recording, watch_rx) = recording_host(&project);
        let control = spawn(
            hub,
            cmd_tx,
            state,
            params,
            config(),
            keys_tx,
            project,
            0,
            recording,
            None,
        );
        (control, watch_rx)
    }

    /// A finished take on disk: one mono WAV + its manifest.
    fn fake_take(project: &Project, take: u32, frames: u32) {
        let dir = project.takes_dir().join(format!("take-{take:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.join("ch01-test.wav"), spec).unwrap();
        for _ in 0..frames {
            writer.write_sample(0.5f32).unwrap();
        }
        writer.finalize().unwrap();
        std::fs::write(
            dir.join("take.toml"),
            format!(
                "schema_version = 1\nstarted_at_unix = 100\nsample_rate = 48000\n\
                 damaged = false\n\n[[tracks]]\nfile = \"ch01-test.wav\"\n\
                 channels = 1\nframes = {frames}\ndropped_samples = 0\n"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn apply_updates_state_and_broadcasts_with_ack() {
        let hub = Hub::new(16);
        let mut hub_rx = hub.subscribe();
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        let ack = WsAck {
            client_id: "tab-1".into(),
            seq: 7,
        };
        control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(0) },
                    level_db: -6.0,
                },
                Some(ack),
            )
            .await
            .unwrap();

        assert_eq!(control.snapshot().await.strips[0].fader_db, -6.0);
        let Ok(EngineCommand::SetParam { target, .. }) = cmd_rx.pop() else {
            panic!("expected a param write");
        };
        assert!((target - db_to_linear(-6.0)).abs() < 1e-6);

        let (channel, payload) = hub_rx.recv().await.unwrap();
        assert_eq!(channel, Channel::Mixer);
        assert!(payload.as_str().contains(r#""type":"state_changed""#));
        assert!(payload.as_str().contains(r#""seq":7"#));
    }

    #[tokio::test]
    async fn a_topology_change_swaps_the_graph_and_snapshots_everyone() {
        let hub = Hub::new(16);
        let mut hub_rx = hub.subscribe();
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        control
            .apply(MixCommand::AddStrip { name: None }, None)
            .await
            .unwrap();

        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::SwapGraph { .. })));
        let (_, payload) = hub_rx.recv().await.unwrap();
        assert!(payload.as_str().contains(r#""type":"mixer_snapshot""#));
        assert_eq!(
            keys_rx.borrow().len(),
            3,
            "two strip meters + master after the add"
        );

        // The NEW strip's fader is live without another recompile.
        control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(1) },
                    level_db: -3.0,
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::SetParam { .. })));
    }

    #[tokio::test]
    async fn a_rejected_command_changes_nothing_and_broadcasts_nothing() {
        let hub = Hub::new(16);
        let mut hub_rx = hub.subscribe();
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state.clone(), params, keys_tx);

        let err = control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(9) },
                    level_db: 0.0,
                },
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, MixError::UnknownTarget(_)));
        assert_eq!(control.snapshot().await, state);
        assert!(cmd_rx.pop().is_err(), "no engine command was pushed");
        assert!(hub_rx.try_recv().is_err(), "nothing was broadcast");
    }

    #[tokio::test]
    async fn an_eq_move_ships_fresh_coefficients() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        control
            .apply(
                MixCommand::SetEqBand {
                    strip: StripId(0),
                    band: trib_core::EqBandKind::Peak,
                    freq_hz: 1_200.0,
                    gain_db: 4.0,
                    q: 0.9,
                },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::SetEqCoeffs { .. })
        ));
    }

    #[tokio::test]
    async fn play_with_no_takes_is_refused() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        let err = control.play().await.unwrap_err();
        assert!(matches!(err, TransportError::NoTake));
        assert!(cmd_rx.pop().is_err(), "no engine command was pushed");
    }

    #[tokio::test]
    async fn play_rolls_the_latest_take_and_stop_parks_the_playhead() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 48_000);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        let dto = control.play().await.unwrap();
        assert_eq!(dto.state, TransportPhase::Playing);
        assert_eq!(dto.take, Some(1));
        assert_eq!(dto.total_frames, 48_000);
        assert_eq!(dto.lanes.len(), 1);
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::StartPlayback { .. })
        ));

        let dto = control.play_stop().await;
        assert_eq!(dto.state, TransportPhase::Stopped);
        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::StopPlayback)));
    }

    #[tokio::test]
    async fn a_stopped_seek_moves_the_playhead_without_engine_traffic() {
        let hub = Hub::new(16);
        let mut hub_rx = hub.subscribe();
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 48_000);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        let dto = control.seek(12_000).await.unwrap();
        assert_eq!(dto.position_frames, 12_000);
        assert!(cmd_rx.pop().is_err(), "no engine command for a parked seek");
        let (channel, payload) = hub_rx.recv().await.unwrap();
        assert_eq!(channel, Channel::Transport);
        assert!(payload.as_str().contains(r#""position_frames":12000"#));

        let dto = control.seek(999_999_999).await.unwrap();
        assert_eq!(
            dto.position_frames, 48_000,
            "seeks clamp to the take length"
        );
    }

    #[tokio::test]
    async fn record_start_auto_stops_playback() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (mut state, _, _) = initial();
        state.strips[0].record_arm = true;
        let compiled = compile(
            &state,
            48_000,
            256,
            &trib_engine::InputSlots::single_default(64),
        );
        let (keys_tx, _keys_rx) = watch::channel(compiled.meter_keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 4_800);
        let control = spawn_test_in(hub, cmd_tx, state, compiled.params, keys_tx, project);

        control.play().await.unwrap();
        let dto = control.record_start().await.unwrap();
        assert_eq!(dto.state, TransportPhase::Recording, "tape wins");
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::StartPlayback { .. })
        ));
        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::StopPlayback)));
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::StartRecord { .. })
        ));
    }

    #[tokio::test]
    async fn lane_gates_ride_the_ring_while_playing_and_survive_in_the_dto() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 48_000);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        control.play().await.unwrap();
        let _ = cmd_rx.pop(); // StartPlayback
        let dto = control.set_lane_gate(0, Some(true), None).await.unwrap();
        assert!(dto.lanes[0].solo && !dto.lanes[0].mute);
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::SetPlaybackSolo { track: 0, on: true })
        ));

        let err = control
            .set_lane_gate(9, None, Some(true))
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::NoSuchLane));

        // Gates persist across a stop — the next session rebuilds from them.
        control.play_stop().await;
        assert!(control.transport().await.lanes[0].solo);
    }

    #[test]
    fn timeline_position_folds_loop_passes_onto_the_take() {
        let region = Some(LoopRegionDto {
            start_frames: 100,
            end_frames: 200,
        });
        assert_eq!(timeline_position(0, None, 250), 250);
        assert_eq!(
            timeline_position(50, region, 40),
            90,
            "linear until the end"
        );
        assert_eq!(timeline_position(50, region, 150), 100, "wraps at the end");
        assert_eq!(timeline_position(50, region, 175), 125);
        assert_eq!(timeline_position(150, region, 350), 100, "many passes fold");
        let degenerate = Some(LoopRegionDto {
            start_frames: 200,
            end_frames: 200,
        });
        assert_eq!(timeline_position(0, degenerate, 300), 300, "no div by zero");
    }

    #[tokio::test]
    async fn a_loop_is_validated_and_rebuilds_a_playing_session() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 48_000);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        let bad = LoopRegionDto {
            start_frames: 10_000,
            end_frames: 10_100,
        };
        assert!(matches!(
            control.set_loop(Some(bad)).await.unwrap_err(),
            TransportError::BadLoop(_)
        ));
        let past_end = LoopRegionDto {
            start_frames: 0,
            end_frames: 96_000,
        };
        assert!(matches!(
            control.set_loop(Some(past_end)).await.unwrap_err(),
            TransportError::BadLoop(_)
        ));

        let good = LoopRegionDto {
            start_frames: 4_800,
            end_frames: 24_000,
        };
        let dto = control.set_loop(Some(good)).await.unwrap();
        assert_eq!(dto.loop_region, Some(good));

        control.play().await.unwrap();
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::StartPlayback { .. })
        ));
        // Changing the loop mid-play tears down and rebuilds.
        let dto = control.set_loop(None).await.unwrap();
        assert_eq!(dto.loop_region, None);
        assert_eq!(dto.state, TransportPhase::Playing);
        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::StopPlayback)));
        assert!(matches!(
            cmd_rx.pop(),
            Ok(EngineCommand::StartPlayback { .. })
        ));
    }

    #[tokio::test]
    async fn play_while_recording_is_refused() {
        let hub = Hub::new(16);
        let (cmd_tx, _cmd_rx) = rtrb::RingBuffer::new(16);
        let (mut state, _, _) = initial();
        state.strips[0].record_arm = true;
        let compiled = compile(
            &state,
            48_000,
            256,
            &trib_engine::InputSlots::single_default(64),
        );
        let (keys_tx, _keys_rx) = watch::channel(compiled.meter_keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take(&project, 1, 4_800);
        let control = spawn_test_in(hub, cmd_tx, state, compiled.params, keys_tx, project);

        control.record_start().await.unwrap();
        let err = control.play().await.unwrap_err();
        assert!(matches!(err, TransportError::Busy(_)));
        let err = control.seek(100).await.unwrap_err();
        assert!(matches!(err, TransportError::Busy(_)));
    }

    /// An armed console ready to record, plus the harness plumbing.
    fn armed() -> (MixerState, ParamMap, Vec<MeterKey>) {
        let (mut state, _, _) = initial();
        state.strips[0].record_arm = true;
        let compiled = compile(
            &state,
            48_000,
            256,
            &trib_engine::InputSlots::single_default(64),
        );
        (state, compiled.params, compiled.meter_keys)
    }

    #[tokio::test]
    async fn a_destination_change_is_refused_while_recording_but_format_is_not() {
        let hub = Hub::new(16);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = armed();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);
        let other = tempfile::tempdir().unwrap();

        control.record_start().await.unwrap();
        let err = control
            .update_recording(Some(other.path().to_path_buf()), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::Busy(_)));

        // Format and rate stay editable mid-take: they only bind at the
        // NEXT record start.
        let dto = control
            .update_recording(None, Some(RecordFormat::Flac24), Some(96_000))
            .await
            .unwrap();
        assert_eq!(dto.format, RecordFormat::Flac24);
        assert_eq!(dto.configured_sample_rate, 96_000);
        assert!(dto.restart_required, "engine still runs 48k");
        control.record_stop().await;
    }

    #[tokio::test]
    async fn a_destination_swap_creates_fresh_tape_seeded_with_the_console() {
        let hub = Hub::new(16);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        let (control, mut watch_rx) =
            spawn_test_watched(hub, cmd_tx, state.clone(), params, keys_tx, project);
        let other = tempfile::tempdir().unwrap();

        let dto = control
            .update_recording(Some(other.path().to_path_buf()), None, None)
            .await
            .unwrap();
        assert_eq!(dto.destination, other.path().display().to_string());
        assert_eq!(dto.project_name, "Session", "fresh tape at the new root");
        assert_eq!(
            dto.configured_destination.as_deref(),
            Some(other.path().to_str().unwrap())
        );

        // The console survives the move: the fresh project was seeded
        // with the running state.
        assert_eq!(control.snapshot().await, state);
        // The API sees the new project through the watch.
        assert!(watch_rx.has_changed().unwrap());
        let published = watch_rx.borrow_and_update().clone();
        assert!(published.dir.starts_with(other.path()));
        // The pref persisted.
        let prefs_text = std::fs::read_to_string(dir.path().join("recording.toml")).unwrap();
        assert!(prefs_text.contains(other.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn a_destination_swap_adopts_the_existing_project_and_its_console() {
        let hub = Hub::new(16);
        let mut hub_rx = hub.subscribe();
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        // The drive already carries a gig with a different console.
        let other = tempfile::tempdir().unwrap();
        let gig_state = MixerState {
            strips: vec![
                StripState::new(StripId(0), "Vox".into()),
                StripState::new(StripId(1), "Git".into()),
            ],
            ..MixerState::default()
        };
        trib_project::create_project(other.path(), "gig", &gig_state).unwrap();

        let dto = control
            .update_recording(Some(other.path().to_path_buf()), None, None)
            .await
            .unwrap();
        assert_eq!(dto.project_name, "gig");
        let snapshot = control.snapshot().await;
        assert_eq!(snapshot.strips.len(), 2, "the gig's console took the desk");
        assert_eq!(snapshot.strips[0].name, "Vox");
        // Adoption is the boot path mid-flight: recompile + snapshot out.
        assert!(matches!(cmd_rx.pop(), Ok(EngineCommand::SwapGraph { .. })));
        let (channel, payload) = hub_rx.recv().await.unwrap();
        assert_eq!(channel, Channel::Mixer);
        assert!(payload.as_str().contains("Vox"));
    }

    #[tokio::test]
    async fn an_unusable_destination_is_refused_and_nothing_changes() {
        let hub = Hub::new(16);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);

        let before = control.recording_settings().await;
        let err = control
            .update_recording(
                Some("/proc/no-such-place".into()),
                Some(RecordFormat::Flac16),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::BadSetting(_)));
        let after = control.recording_settings().await;
        assert_eq!(after.destination, before.destination);
        assert_eq!(
            after.format,
            RecordFormat::Wav32Float,
            "the PUT is atomic: a bad destination applies nothing"
        );

        let err = control
            .update_recording(None, None, Some(12_345))
            .await
            .unwrap_err();
        assert!(matches!(err, TransportError::BadSetting(_)));
    }

    #[tokio::test]
    async fn the_next_take_lands_on_the_new_destination_in_the_new_format() {
        let hub = Hub::new(16);
        let (cmd_tx, mut cmd_rx) = rtrb::RingBuffer::new(16);
        let (state, params, keys) = armed();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let (control, _dir) = spawn_test(hub, cmd_tx, state, params, keys_tx);
        let other = tempfile::tempdir().unwrap();

        control
            .update_recording(
                Some(other.path().to_path_buf()),
                Some(RecordFormat::Flac16),
                None,
            )
            .await
            .unwrap();
        control.record_start().await.unwrap();
        // Dropping the engine's record set drops the ring producers — the
        // writer finalizes an (empty) take.
        let Ok(EngineCommand::StartRecord { set }) = cmd_rx.pop() else {
            panic!("no record set reached the engine");
        };
        drop(set);
        control.record_stop().await;

        let take_dir = trib_project::load_latest(other.path())
            .expect("the swap created a project")
            .0
            .takes_dir()
            .join("take-001");
        // The writer joins off-loop; poll briefly for the manifest.
        let manifest_path = take_dir.join("take.toml");
        for _ in 0..100 {
            if manifest_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let manifest = std::fs::read_to_string(&manifest_path).expect("take.toml lands");
        assert!(manifest.contains("format = \"flac16\""));
        assert!(manifest.contains("ch01-ch-1.flac"), "{manifest}");
    }
}
