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
    /// A take number that is not in the open session — distinct from
    /// `NoTake`, which is "there are none at all".
    NoSuchTake,
    /// A lane index past the take's tracks.
    NoSuchLane,
    /// A session id that does not name a session under the active root.
    NoSuchSession,
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

/// Which session is open. The id is its directory name — stable across a
/// rename, because a rename never moves the directory.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRef {
    pub id: String,
    pub name: String,
}

impl SessionRef {
    fn of(project: &Project) -> Self {
        SessionRef {
            id: project
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            name: project.name.clone(),
        }
    }
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
    /// Point the transport at a take of the open session.
    SelectTake {
        take: u32,
        reply: oneshot::Sender<Result<TransportDto, TransportError>>,
    },
    /// Remove one take from the open session, permanently.
    DeleteTake {
        take: u32,
        reply: oneshot::Sender<Result<(), TransportError>>,
    },
    /// Tear off fresh tape in the active root and put it on the machine.
    CreateSession {
        name: String,
        seed: crate::console::SessionSeed,
        reply: oneshot::Sender<Result<SessionRef, TransportError>>,
    },
    /// Put a different existing session on the machine.
    OpenSession {
        id: String,
        reply: oneshot::Sender<Result<SessionRef, TransportError>>,
    },
    /// Retitle a session — the open one or any other in the active root.
    RenameSession {
        id: String,
        name: String,
        reply: oneshot::Sender<Result<SessionRef, TransportError>>,
    },
    /// Permanently delete a session that is not the open one.
    DeleteSession {
        id: String,
        reply: oneshot::Sender<Result<(), TransportError>>,
    },
    /// Which session is open, for the sessions listing and the tape label.
    Session {
        reply: oneshot::Sender<SessionRef>,
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
    /// From the instrument host: a rack has finished loading. Handed
    /// straight to the engine — unlike a slot map it needs no recompile,
    /// because instrument patches resolve by position and the positions
    /// come from the document the host was given.
    InstrumentRackChanged {
        rack: Box<trib_engine::InstrumentRack>,
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

    pub async fn select_take(&self, take: u32) -> Result<TransportDto, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::SelectTake { take, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn delete_take(&self, take: u32) -> Result<(), TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::DeleteTake { take, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn create_session(
        &self,
        name: String,
        seed: crate::console::SessionSeed,
    ) -> Result<SessionRef, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::CreateSession { name, seed, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn open_session(&self, id: String) -> Result<SessionRef, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::OpenSession { id, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn rename_session(
        &self,
        id: String,
        name: String,
    ) -> Result<SessionRef, TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::RenameSession { id, name, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    pub async fn delete_session(&self, id: String) -> Result<(), TransportError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::DeleteSession { id, reply })
            .await
            .expect("control task outlives its callers");
        response.await.expect("control task replies")
    }

    /// Stand in for the writer-join task, which is what normally reports a
    /// finished take. Tests need to move the shelf without recording.
    #[cfg(test)]
    async fn take_finalized(&self) {
        self.tx
            .send(ControlMsg::TakeFinalized)
            .await
            .expect("control task outlives its callers");
        // The message is fire-and-forget; a round trip through a replying
        // message is how we know it has been handled.
        let _ = self.transport().await;
    }

    /// Which session is open. Cheap — no disk.
    pub async fn session(&self) -> SessionRef {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(ControlMsg::Session { reply })
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

    /// Called from the instrument host thread once a rack has loaded.
    pub fn instrument_rack_changed_blocking(&self, rack: Box<trib_engine::InstrumentRack>) {
        let _ = self
            .tx
            .blocking_send(ControlMsg::InstrumentRackChanged { rack });
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
    instruments: Option<crate::instrument_host::InstrumentHandle>,
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
        instruments,
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

/// The take the transport points at: what PLAY rolls, what `lanes` gate,
/// what the ruler measures. Cached so every transport reply doesn't
/// re-read disk. Defaults to the newest and follows every new take; the
/// console can point it anywhere in the open session.
#[derive(Clone)]
struct SelectedTake {
    take: u32,
    sample_rate: u32,
    format: trib_project::RecordFormat,
    total_frames: u64,
    tracks: Vec<TakeTrackInfo>,
}

/// Pure over a manifest, so the shape is testable without disk.
fn take_summary(info: trib_project::TakeInfo) -> SelectedTake {
    SelectedTake {
        take: info.take,
        sample_rate: info.sample_rate,
        format: info.format,
        total_frames: info.tracks.iter().map(|t| t.frames).max().unwrap_or(0),
        tracks: info.tracks,
    }
}

fn load_latest_take(project: &Project) -> Option<SelectedTake> {
    list_takes(project).into_iter().next().map(take_summary)
}

fn load_take(project: &Project, take: u32) -> Option<SelectedTake> {
    list_takes(project)
        .into_iter()
        .find(|info| info.take == take)
        .map(take_summary)
}

/// All transport state, bundled so the helpers stay readable.
struct TransportCtl {
    state: TransportState,
    /// What PLAY would roll. Not necessarily the newest — the console can
    /// point this at any take of the open session.
    selected: Option<SelectedTake>,
    /// The newest finished take on the shelf, so a client can say "you are
    /// not on the newest" without holding the whole list.
    latest_take: Option<u32>,
    /// Playback gates, index-aligned with the SELECTED take's tracks.
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
        let selected = load_latest_take(project);
        let lanes = vec![LaneDto::default(); selected.as_ref().map_or(0, |s| s.tracks.len())];
        TransportCtl {
            state: TransportState::Stopped,
            latest_take: selected.as_ref().map(|s| s.take),
            selected,
            lanes,
            loop_region: None,
            stopped_position: 0,
            generation: 0,
            engine_rate,
            monitor: MonitorTarget::Hardware,
            monitor_generation,
        }
    }

    /// Point the transport at a take.
    ///
    /// A different tape resets the review posture: gates open, loop gone,
    /// tape rewound. `lanes` is index-aligned with the take's tracks and
    /// `loop_region`'s frames are only valid against its length, so those
    /// three cannot move independently without lying about each other.
    fn select(&mut self, selected: Option<SelectedTake>) {
        self.lanes = vec![LaneDto::default(); selected.as_ref().map_or(0, |s| s.tracks.len())];
        self.selected = selected;
        self.loop_region = None;
        self.stopped_position = 0;
    }

    /// Point at the newest take and re-read the shelf's high mark. The one
    /// reset point for "the shelf moved" — a finished take, or a different
    /// session opening.
    fn select_latest(&mut self, project: &Project) {
        let latest = load_latest_take(project);
        self.latest_take = latest.as_ref().map(|s| s.take);
        self.select(latest);
    }

    /// Re-read the high mark only; the selection is untouched.
    fn refresh_latest_number(&mut self, project: &Project) {
        self.latest_take = list_takes(project).first().map(|info| info.take);
    }

    fn recording(&self) -> bool {
        matches!(self.state, TransportState::Recording(_))
    }

    fn dto(&self) -> TransportDto {
        // Every field that describes "the tape on the machine" is decided
        // inside this match, together. They used to be split: `lanes`,
        // `total_frames` and `sample_rate` were computed from the selected
        // take even while RECORDING, so during a take they described the
        // PREVIOUS one — a 44.1 kHz take selected before hitting REC made
        // the recording counter run ~9% slow, and the lane gates addressed
        // a playback set that did not exist.
        let (phase, take, started_at_unix, position_frames, lanes, total_frames, sample_rate) =
            match &self.state {
                TransportState::Stopped => (
                    TransportPhase::Stopped,
                    self.selected.as_ref().map(|s| s.take),
                    None,
                    self.stopped_position,
                    self.lanes.clone(),
                    self.selected.as_ref().map_or(0, |s| s.total_frames),
                    self.selected
                        .as_ref()
                        .map_or(self.engine_rate, |s| s.sample_rate),
                ),
                TransportState::Playing(s) => (
                    TransportPhase::Playing,
                    self.selected.as_ref().map(|sel| sel.take),
                    None,
                    timeline_position(
                        s.start_frame,
                        s.loop_region,
                        s.shared.position.load(Ordering::Relaxed),
                    ),
                    self.lanes.clone(),
                    self.selected.as_ref().map_or(0, |sel| sel.total_frames),
                    self.selected
                        .as_ref()
                        .map_or(self.engine_rate, |sel| sel.sample_rate),
                ),
                // Recording describes the take being CUT: no playback
                // gates, no known length yet, and the engine's own rate.
                TransportState::Recording(r) => (
                    TransportPhase::Recording,
                    Some(r.take),
                    Some(r.started_at_unix),
                    0,
                    Vec::new(),
                    0,
                    self.engine_rate,
                ),
            };
        TransportDto {
            state: phase,
            take,
            latest_take: self.latest_take,
            started_at_unix,
            position_frames,
            sample_rate,
            engine_sample_rate: self.engine_rate,
            total_frames,
            loop_region: self.loop_region,
            monitor: self.monitor,
            lanes,
        }
    }
}

/// The device identities the console references — what the orchestrator
/// must keep open. `None` = the system default input.
///
/// Instrument patches are skipped rather than represented here: the device
/// orchestrator opens streams, reconciles renames and allocates frame
/// slots, and an instrument has none of those. Its lifecycle belongs to the
/// instrument host.
fn wanted_devices(state: &MixerState) -> std::collections::BTreeSet<Option<String>> {
    state
        .strips
        .iter()
        .filter_map(|s| s.input.as_ref())
        .filter_map(|a| a.device_name())
        .map(|device| device.map(str::to_owned))
        .collect()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_secs()
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
    instruments: Option<crate::instrument_host::InstrumentHandle>,
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
    // Same for the rack: the loaded session may already carry instruments,
    // and they must be loading before anyone presses a key.
    let mut last_instruments = state.instruments.clone();
    if let Some(instruments) = &instruments {
        instruments.instruments_changed(last_instruments.clone());
    }
    let mut save_generation: u64 = 0;
    // Borrowing thirteen loop locals at a call site is unreadable and
    // borrow-checks badly if hoisted into a variable, so the bundle is
    // built fresh at each use. A macro rather than a function because
    // every field is a `&mut` into this scope.
    macro_rules! swap_ctx {
        () => {
            SwapCtx {
                hub: &hub,
                cmd_tx: &mut cmd_tx,
                state: &mut state,
                params: &mut params,
                config: &config,
                meter_keys: &meter_keys,
                input_slots: &input_slots,
                last_wanted: &mut last_wanted,
                devices: &devices,
                last_instruments: &mut last_instruments,
                instruments: &instruments,
                project: &mut project,
                project_created_at: &mut project_created_at,
                save_generation: &mut save_generation,
                recording: &mut recording,
                ctl: &mut ctl,
            }
        };
    }
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
                    // The rack is rebuilt from the document itself rather
                    // than from a diff: what the host must load is exactly
                    // what the console says, and comparing whole documents
                    // is cheaper than deciding which field mattered.
                    if state.instruments != last_instruments {
                        last_instruments = state.instruments.clone();
                        if let Some(instruments) = &instruments {
                            instruments.instruments_changed(last_instruments.clone());
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
                        let total = ctl.selected.as_ref().map_or(0, |s| s.total_frames);
                        ctl.stopped_position = frames.min(total);
                        publish_transport(&hub, &ctl);
                        Ok(ctl.dto())
                    }
                    // Seek = teardown + rebuild at the target: no stale
                    // samples by construction.
                    TransportState::Playing(_) => {
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                        let total = ctl.selected.as_ref().map_or(0, |s| s.total_frames);
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
                    let Some(latest) = &ctl.selected else {
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
                // Gates address the SELECTED take's playback set, and
                // while tape rolls there is none. Before this guard the
                // handler indexed whatever lanes were left over from the
                // previous take and pushed a solo at a session that did
                // not exist.
                let result = if ctl.recording() {
                    Err(TransportError::Busy("no playback gates while recording"))
                } else if (track as usize) >= ctl.lanes.len() {
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
                ctl.select_latest(&project);
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
            ControlMsg::InstrumentRackChanged { rack } => {
                // Safe to install mid-take, unlike a slot map: the rack
                // replaces synthesisers, not record taps, and the take
                // keeps writing whatever they produce. A player who fixes
                // a wrong preset halfway through a song should hear the
                // fix, not wait for the tape to stop.
                push(&mut cmd_tx, EngineCommand::SwapRack { rack });
            }
            ControlMsg::SelectTake { take, reply } => {
                let result = 'select: {
                    // While tape rolls, `take` IS the take being cut;
                    // pointing the selection elsewhere would make take,
                    // lanes and total_frames describe two tapes at once.
                    if ctl.recording() {
                        break 'select Err(TransportError::Busy(
                            "no take switching while recording",
                        ));
                    }
                    // Already there: leave the review posture alone. A row
                    // is easy to double-tap on a phone, and losing a
                    // hand-drawn loop to a fat finger is the worse outcome.
                    if ctl.selected.as_ref().is_some_and(|s| s.take == take) {
                        break 'select Ok(ctl.dto());
                    }
                    let Some(selected) = load_take(&project, take) else {
                        break 'select Err(TransportError::NoSuchTake);
                    };
                    // Playback holds this session's file descriptors open.
                    if matches!(ctl.state, TransportState::Playing(_)) {
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                    }
                    // A rate mismatch does NOT block selection: the
                    // waveform, length and track names are all true, and
                    // refusing to show it would make an unplayable take
                    // indistinguishable from one that isn't there. PLAY is
                    // where the refusal belongs.
                    ctl.select(Some(selected));
                    publish_transport(&hub, &ctl);
                    Ok(ctl.dto())
                };
                let _ = reply.send(result);
            }
            ControlMsg::DeleteTake { take, reply } => {
                let result = 'del: {
                    if ctl.recording() {
                        break 'del Err(TransportError::Busy(
                            "stop recording before deleting a take",
                        ));
                    }
                    // The feeder threads hold open readers and reopen the
                    // files on every loop pass, so unlinking underneath
                    // them would run playback off unlinked inodes until a
                    // later wrap died somewhere confusing.
                    if matches!(ctl.state, TransportState::Playing(_))
                        && ctl.selected.as_ref().is_some_and(|s| s.take == take)
                    {
                        break 'del Err(TransportError::Busy(
                            "stop playback before deleting the take you're playing",
                        ));
                    }
                    if load_take(&project, take).is_none() {
                        break 'del Err(TransportError::NoSuchTake);
                    }
                    if let Err(e) = trib_project::delete_take(&project, take) {
                        break 'del Err(TransportError::Io(e.to_string()));
                    }
                    // Deleting what we were reviewing falls back to the
                    // newest remaining; deleting anything else leaves the
                    // selection where it is.
                    if ctl.selected.as_ref().is_some_and(|s| s.take == take) {
                        ctl.select_latest(&project);
                    } else {
                        ctl.refresh_latest_number(&project);
                    }
                    publish_transport(&hub, &ctl);
                    publish_takes(&hub, &project);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            ControlMsg::Session { reply } => {
                let _ = reply.send(SessionRef::of(&project));
            }
            ControlMsg::CreateSession { name, seed, reply } => {
                let result = 'create: {
                    if ctl.recording() {
                        break 'create Err(TransportError::Busy(
                            "stop recording before starting a new session",
                        ));
                    }
                    // The desk the new session starts from is the user's
                    // choice: a clean template, the same channel mapping,
                    // or an exact copy of what is running.
                    let console = crate::console::seed_console(seed, &state);
                    let root = recording.active_root.clone();
                    let made = tokio::task::spawn_blocking(move || {
                        trib_project::create_project(&root, &name, &console)
                            .and_then(|p| trib_project::manifest_of(&p).map(|m| (p, m)))
                    })
                    .await
                    .expect("session task is never cancelled");
                    let (new_project, manifest) = match made {
                        Ok(made) => made,
                        Err(e) => break 'create Err(session_error(e)),
                    };
                    if matches!(ctl.state, TransportState::Playing(_)) {
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                    }
                    // Always an adopt: under Template or Mapping the new
                    // manifest's console differs from the running desk, and
                    // under Console the swap is simply a no-op on it. One
                    // route beats a second branch that could drift.
                    adopt_project(swap_ctx!(), new_project, manifest, true);
                    publish_sessions(&hub, &project);
                    publish_takes(&hub, &project);
                    Ok(SessionRef::of(&project))
                };
                let _ = reply.send(result);
            }
            ControlMsg::OpenSession { id, reply } => {
                let result = 'open: {
                    if ctl.recording() {
                        break 'open Err(TransportError::Busy(
                            "stop recording before switching sessions",
                        ));
                    }
                    // Already open: harmless, and must not cost the review
                    // posture or a needless graph swap.
                    if SessionRef::of(&project).id == id {
                        break 'open Ok(SessionRef::of(&project));
                    }
                    let root = recording.active_root.clone();
                    let opened = tokio::task::spawn_blocking(move || {
                        trib_project::resolve(&root, &id)
                            .and_then(|dir| trib_project::open_session(&dir))
                    })
                    .await
                    .expect("session task is never cancelled");
                    let (new_project, manifest) = match opened {
                        Ok(opened) => opened,
                        Err(e) => break 'open Err(session_error(e)),
                    };
                    if matches!(ctl.state, TransportState::Playing(_)) {
                        stop_playback(&mut ctl, &mut cmd_tx, &hub);
                    }
                    // Opening a session is always an adopt: its console IS
                    // the session.
                    adopt_project(swap_ctx!(), new_project, manifest, true);
                    publish_sessions(&hub, &project);
                    publish_takes(&hub, &project);
                    Ok(SessionRef::of(&project))
                };
                let _ = reply.send(result);
            }
            ControlMsg::RenameSession { id, name, reply } => {
                // Allowed while recording: this rewrites a name, not audio,
                // and scribbling on the tape while it rolls is exactly the
                // gesture the label was designed for.
                let result = 'rename: {
                    let open = SessionRef::of(&project);
                    if open.id == id {
                        let name = match trib_project::validate_name(&name) {
                            Ok(name) => name.to_owned(),
                            Err(e) => break 'rename Err(session_error(e)),
                        };
                        project.name = name;
                        if let Err(e) = save_manifest(&project, &state, project_created_at) {
                            break 'rename Err(TransportError::Io(e.to_string()));
                        }
                        let _ = recording.project_watch.send(Arc::new(project.clone()));
                        publish_sessions(&hub, &project);
                        break 'rename Ok(SessionRef::of(&project));
                    }
                    // Renaming a session we do not have open: the manifest
                    // is rewritten in place and the desk is untouched.
                    let root = recording.active_root.clone();
                    let id_for_reply = id.clone();
                    let renamed = tokio::task::spawn_blocking(move || {
                        trib_project::resolve(&root, &id)
                            .and_then(|dir| trib_project::rename_session(&dir, &name))
                    })
                    .await
                    .expect("session task is never cancelled");
                    let manifest = match renamed {
                        Ok(manifest) => manifest,
                        Err(e) => break 'rename Err(session_error(e)),
                    };
                    publish_sessions(&hub, &project);
                    Ok(SessionRef {
                        id: id_for_reply,
                        name: manifest.name,
                    })
                };
                let _ = reply.send(result);
            }
            ControlMsg::DeleteSession { id, reply } => {
                let result = 'delete: {
                    if ctl.recording() {
                        break 'delete Err(TransportError::Busy(
                            "stop recording before deleting a session",
                        ));
                    }
                    // Refusing to delete the OPEN session is not fussiness.
                    // One gesture would otherwise make two state changes,
                    // the second irreversible — and if it were the only
                    // session, "delete" would have to create one. It also
                    // subsumes two guards for free: playback holds file
                    // descriptors only under the open project, and in-
                    // flight autosaves target only the open project.
                    if SessionRef::of(&project).id == id {
                        break 'delete Err(TransportError::Busy(
                            "open a different session before deleting this one",
                        ));
                    }
                    let root = recording.active_root.clone();
                    let deleted = tokio::task::spawn_blocking(move || {
                        trib_project::resolve(&root, &id).and_then(trib_project::delete_session)
                    })
                    .await
                    .expect("session task is never cancelled");
                    if let Err(e) = deleted {
                        break 'delete Err(session_error(e));
                    }
                    publish_sessions(&hub, &project);
                    Ok(())
                };
                let _ = reply.send(result);
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
                        // The destination's own console takes over the desk
                        // when it had one; otherwise the manifest we just
                        // wrote IS this console, so there is nothing to swap.
                        adopt_project(
                            SwapCtx {
                                hub: &hub,
                                cmd_tx: &mut cmd_tx,
                                state: &mut state,
                                params: &mut params,
                                config: &config,
                                meter_keys: &meter_keys,
                                input_slots: &input_slots,
                                last_wanted: &mut last_wanted,
                                devices: &devices,
                                last_instruments: &mut last_instruments,
                                instruments: &instruments,
                                project: &mut project,
                                project_created_at: &mut project_created_at,
                                save_generation: &mut save_generation,
                                recording: &mut recording,
                                ctl: &mut ctl,
                            },
                            new_project,
                            manifest,
                            adopted,
                        );
                        recording.prefs.destination = Some(new_root.clone());
                        recording.active_root = new_root;
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

/// Map a session-layer failure onto the transport vocabulary the API
/// already knows how to turn into a status code.
fn session_error(e: trib_project::ProjectError) -> TransportError {
    use trib_project::ProjectError;
    match e {
        ProjectError::BadId | ProjectError::NotASession => TransportError::NoSuchSession,
        ProjectError::BadName(why) => TransportError::BadSetting(why.to_owned()),
        ProjectError::Exists => TransportError::Busy("a session of that name already exists"),
        other => TransportError::Io(other.to_string()),
    }
}

/// Push the open session's take list. Called wherever the shelf moves — a
/// finished take, a deleted one, or a different session opening.
fn publish_takes(hub: &Hub, project: &Project) {
    hub.publish(
        Channel::Transport,
        &ServerMessage::TakesChanged {
            takes: crate::api::takes::take_dtos(project),
        },
    );
}

/// Push which session is open. Carries only the open one: a client showing
/// the list refetches, and building the list here would mean a directory
/// walk on the control task.
fn publish_sessions(hub: &Hub, project: &Project) {
    let open = SessionRef::of(project);
    hub.publish(
        Channel::Sessions,
        &ServerMessage::SessionsChanged {
            open_id: open.id,
            open_name: open.name,
        },
    );
}

/// Everything a project swap touches, borrowed from the control loop.
///
/// Bundled into one struct so `adopt_project` is a single call rather than
/// a thirteen-argument function — every field here is a loop local that a
/// swap genuinely has to move.
struct SwapCtx<'a> {
    hub: &'a Hub,
    cmd_tx: &'a mut Producer<EngineCommand>,
    state: &'a mut MixerState,
    params: &'a mut ParamMap,
    config: &'a EngineConfig,
    meter_keys: &'a watch::Sender<Vec<MeterKey>>,
    input_slots: &'a trib_engine::InputSlots,
    last_wanted: &'a mut std::collections::BTreeSet<Option<String>>,
    devices: &'a Option<crate::device_host::DeviceHandle>,
    last_instruments: &'a mut Vec<trib_core::InstrumentState>,
    instruments: &'a Option<crate::instrument_host::InstrumentHandle>,
    project: &'a mut Project,
    project_created_at: &'a mut u64,
    save_generation: &'a mut u64,
    recording: &'a mut RecordingHost,
    ctl: &'a mut TransportCtl,
}

/// Put a different tape on the machine.
///
/// THE one live-swap sequence: every path that changes which project is
/// open comes through here, so a destination change, a session switch and
/// a session create cannot drift apart.
///
/// `adopted` means the incoming manifest's console replaces the running
/// desk. True for a session switch (you are opening someone else's desk)
/// and for an explicit create under `Template`/`Mapping`; false only when
/// the manifest was just written FROM the running console, where replacing
/// it would be a no-op.
///
/// The caller must have stopped playback first — the outgoing project's
/// feeder threads hold open file descriptors under it.
fn adopt_project(cx: SwapCtx<'_>, new_project: Project, manifest: ProjectManifest, adopted: bool) {
    // Flush the desk we are leaving, synchronously, BEFORE rebinding.
    // Autosave is debounced by 500 ms and the generation bump below
    // cancels whatever is pending, so without this an edit made in the
    // half-second before a swap is simply lost — nudge a fader, switch
    // session, come back, and the nudge is gone. A failed save (drive
    // yanked) must not block the switch: log it and carry on.
    if let Err(e) = save_manifest(cx.project, cx.state, *cx.project_created_at) {
        tracing::error!(%e, dir = %cx.project.dir.display(), "could not save the outgoing session");
    }

    if adopted {
        *cx.state = manifest.mixer;
        let wanted = wanted_devices(cx.state);
        if wanted != *cx.last_wanted {
            *cx.last_wanted = wanted.clone();
            if let Some(devices) = cx.devices {
                devices.wanted_changed(wanted);
            }
        }
        // The incoming session brings its own rack. Told unconditionally
        // rather than on a diff: an empty document must still reach the
        // host, or the previous session's instruments keep sounding.
        *cx.last_instruments = cx.state.instruments.clone();
        if let Some(instruments) = cx.instruments {
            instruments.instruments_changed(cx.last_instruments.clone());
        }
        let compiled = compile(
            cx.state,
            cx.config.sample_rate,
            cx.config.block_size,
            cx.input_slots,
        );
        *cx.params = compiled.params;
        let _ = cx.meter_keys.send(compiled.meter_keys);
        push(
            cx.cmd_tx,
            EngineCommand::SwapGraph {
                graph: compiled.graph,
            },
        );
        cx.hub.publish(
            Channel::Mixer,
            &ServerMessage::MixerSnapshot {
                state: cx.state.clone(),
            },
        );
    }

    // Fences the debounced autosave: the flush above wrote the outgoing
    // manifest exactly once, and this guarantees not twice.
    *cx.save_generation += 1;
    *cx.project = new_project;
    *cx.project_created_at = manifest.created_at_unix;
    let _ = cx
        .recording
        .project_watch
        .send(Arc::new(cx.project.clone()));
    // A different tape resets the review posture: gates open, loop gone,
    // playhead rewound, pointed at this session's newest take.
    cx.ctl.select_latest(cx.project);
    publish_transport(cx.hub, cx.ctl);
}

/// Blocking: ensure the destination exists and is writable, then adopt its
/// newest project — or tear off fresh tape seeded with the current console.
fn open_destination(
    root: &Path,
    seed: &MixerState,
) -> Result<(Project, ProjectManifest, bool), TransportError> {
    std::fs::create_dir_all(root)
        .map_err(|e| TransportError::BadSetting(format!("{}: {e}", root.display())))?;
    // Same probe the destination list uses, so a tile that says "ready"
    // and a save that succeeds can never disagree.
    if !crate::destinations::writable(root) {
        return Err(TransportError::BadSetting(format!(
            "{} is not writable",
            root.display()
        )));
    }
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
    let latest = ctl.selected.clone().ok_or(TransportError::NoTake)?;
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
                    trib_project::file_slug(&strip.name),
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

    // The notes, beside the audio. Only worth a ring when an armed strip
    // is actually fed by an instrument: a session of microphones should
    // not carry an empty `.mid` around.
    let instrument_slots: Vec<(u16, String)> = {
        let mut base = 0u16;
        state
            .instruments
            .iter()
            .flat_map(|inst| {
                let names = inst.channel_names();
                let slots: Vec<(u16, String)> = (0..inst.channels_slots())
                    .map(|slot| {
                        (
                            base + slot as u16,
                            // A split's own name; a stereo instrument's is
                            // just the instrument.
                            if inst.splits.is_empty() {
                                inst.name.clone()
                            } else {
                                names
                                    .get(slot)
                                    .cloned()
                                    .unwrap_or_else(|| inst.name.clone())
                            },
                        )
                    })
                    .collect();
                base += inst.channels_slots() as u16;
                slots
            })
            .collect()
    };
    let armed_instruments: std::collections::BTreeSet<u16> = state
        .strips
        .iter()
        .filter(|strip| strip.record_arm)
        .filter_map(|strip| strip.input.as_ref())
        .filter_map(|assign| {
            let id = assign.instrument_id()?;
            let position = state.instruments.iter().position(|i| i.id == id)?;
            let base: usize = state.instruments[..position]
                .iter()
                .map(|i| i.channels_slots())
                .sum();
            // A stereo instrument is one slot; a split kit's channel index
            // IS its slot.
            let slot = if state.instruments[position].splits.is_empty() {
                base
            } else {
                base + assign.channel() as usize
            };
            u16::try_from(slot).ok()
        })
        .collect();
    let midi_sink = (!armed_instruments.is_empty()).then(|| {
        let (tx, rx) = RingBuffer::new(trib_engine::MIDI_CAPTURE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let capture = Box::new(trib_engine::MidiCapture {
            tx,
            dropped: dropped.clone(),
            start_sample: 0,
        });
        let voices = instrument_slots
            .iter()
            .filter(|(slot, _)| armed_instruments.contains(slot))
            .map(|(slot, name)| (*slot, name.clone()))
            .collect();
        (
            capture,
            trib_project::MidiSink {
                rx,
                dropped,
                voices,
            },
        )
    });
    let (midi_capture, midi_sink) = match midi_sink {
        Some((capture, sink)) => (Some(capture), Some(sink)),
        None => (None, None),
    };

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
        midi_sink,
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
                midi: midi_capture,
            }),
        },
    );
    // The previous take's gates describe a playback set that is gone.
    // `dto()` reports empty lanes while recording anyway; clearing here
    // means nothing stale survives to be reapplied at the next build.
    ctl.lanes.clear();
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
        // A performance edit is a ParamOnly change, but it does not land in
        // the graph's parameter table at all — it retunes a synth, which
        // the instrument host owns. It reaches the rack through the rebuild
        // the host performs, not through this dispatch.
        StateDelta::InstrumentChanged { .. } => vec![],
        // Topology deltas never reach here.
        StateDelta::Input { .. }
        | StateDelta::StripAdded { .. }
        | StateDelta::StripRemoved { .. }
        | StateDelta::Route { .. }
        | StateDelta::BusAdded { .. }
        | StateDelta::BusRemoved { .. }
        | StateDelta::InstrumentAdded { .. }
        | StateDelta::InstrumentRemoved { .. }
        | StateDelta::InstrumentStripsAdded { .. } => {
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
            None,
        );
        (control, watch_rx)
    }

    /// A finished take on disk: one mono WAV + its manifest.
    fn fake_take(project: &Project, take: u32, frames: u32) {
        fake_take_at(project, take, frames, 48_000);
    }

    fn fake_take_at(project: &Project, take: u32, frames: u32, rate: u32) {
        let dir = project.takes_dir().join(format!("take-{take:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
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
                "schema_version = 1\nstarted_at_unix = 100\nsample_rate = {rate}\n\
                 damaged = false\n\n[[tracks]]\nfile = \"ch01-test.wav\"\n\
                 channels = 1\nframes = {frames}\ndropped_samples = 0\n"
            ),
        )
        .unwrap();
    }

    /// A session with three takes on the shelf, newest = 3.
    async fn shelf() -> (ControlHandle, tempfile::TempDir) {
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        for take in 1..=3 {
            fake_take(&project, take, 48_000 * take);
        }
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);
        (control, dir)
    }

    #[tokio::test]
    async fn a_take_can_be_selected_and_it_is_what_play_rolls() {
        let (control, _dir) = shelf().await;
        // Boots pointed at the newest.
        let dto = control.transport().await;
        assert_eq!(dto.take, Some(3));
        assert_eq!(dto.latest_take, Some(3));

        let dto = control.select_take(1).await.unwrap();
        assert_eq!(dto.take, Some(1), "selection moved");
        assert_eq!(dto.latest_take, Some(3), "the shelf did not");
        assert_eq!(dto.total_frames, 48_000, "the ruler measures take 1");

        // And PLAY rolls what is selected, not the newest.
        let played = control.play().await.unwrap();
        assert_eq!(played.take, Some(1));
    }

    /// A different tape resets the review posture. `lanes` is index-aligned
    /// with the take's tracks and a loop's frames are only valid against
    /// its length, so these cannot move independently.
    #[tokio::test]
    async fn selecting_a_different_take_resets_gates_loop_and_playhead() {
        let (control, _dir) = shelf().await;
        control.set_lane_gate(0, Some(true), None).await.unwrap();
        control
            .set_loop(Some(LoopRegionDto {
                start_frames: 0,
                end_frames: 24_000,
            }))
            .await
            .unwrap();
        control.seek(12_000).await.unwrap();

        let dto = control.select_take(2).await.unwrap();
        assert!(!dto.lanes[0].solo, "gates open");
        assert!(dto.loop_region.is_none(), "loop gone");
        assert_eq!(dto.position_frames, 0, "tape rewound");
    }

    /// A row is easy to double-tap on a phone; losing a hand-drawn loop to
    /// a fat finger is the worse outcome.
    #[tokio::test]
    async fn re_selecting_the_open_take_leaves_the_posture_alone() {
        let (control, _dir) = shelf().await;
        control
            .set_loop(Some(LoopRegionDto {
                start_frames: 0,
                end_frames: 24_000,
            }))
            .await
            .unwrap();
        let dto = control.select_take(3).await.unwrap();
        assert!(dto.loop_region.is_some(), "the loop survived");
    }

    #[tokio::test]
    async fn selecting_a_take_that_is_not_there_is_not_found() {
        let (control, _dir) = shelf().await;
        assert!(matches!(
            control.select_take(99).await,
            Err(TransportError::NoSuchTake)
        ));
        // And the selection did not move.
        assert_eq!(control.transport().await.take, Some(3));
    }

    /// A take cut at another rate still has a true waveform, length and
    /// track names. Refusing to SHOW it would make it indistinguishable
    /// from a take that is not there; PLAY is where the refusal belongs.
    #[tokio::test]
    async fn a_take_cut_at_another_rate_selects_but_will_not_play() {
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take_at(&project, 1, 44_100, 44_100);
        fake_take(&project, 2, 48_000);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        let dto = control.select_take(1).await.unwrap();
        assert_eq!(dto.take, Some(1), "selectable");
        assert_eq!(dto.sample_rate, 44_100, "its own rate");
        assert_eq!(
            dto.engine_sample_rate, 48_000,
            "and the engine's, to compare"
        );
        assert!(matches!(
            control.play().await,
            Err(TransportError::SampleRateMismatch)
        ));
    }

    /// Pressing REC already replaces the Tracks room with the live
    /// document, so the user's place is gone at REC, not at finalize.
    /// Snapping to what you just cut continues what they are watching.
    #[tokio::test]
    async fn a_finished_take_becomes_the_selection_even_after_browsing() {
        let (control, dir) = shelf().await;
        control.select_take(1).await.unwrap();
        // Simulate the writer finishing take 4.
        let project = trib_project::load_latest(dir.path()).unwrap().0;
        fake_take(&project, 4, 4_800);
        control.take_finalized().await;
        let dto = control.transport().await;
        assert_eq!(dto.take, Some(4));
        assert_eq!(dto.latest_take, Some(4));
    }

    /// The bug, encoded. `lanes`, `total_frames` and `sample_rate` used to
    /// be computed outside the phase match, so during a take they all
    /// described the PREVIOUS one — a 44.1 kHz take selected before REC
    /// made the recording counter run ~9% slow, and the gates addressed a
    /// playback set that did not exist.
    #[tokio::test]
    async fn recording_reports_its_own_tape_not_the_previous_take() {
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = armed();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "test", &state).unwrap();
        fake_take_at(&project, 1, 44_100, 44_100);
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);
        assert_eq!(control.transport().await.sample_rate, 44_100);

        control.record_start().await.unwrap();
        let dto = control.transport().await;
        assert_eq!(dto.sample_rate, 48_000, "the ENGINE's rate, not the take's");
        assert_eq!(dto.total_frames, 0, "the new take has no known length yet");
        assert!(dto.lanes.is_empty(), "no playback set to gate");
        // And a gate is refused rather than silently addressing the old take.
        assert!(matches!(
            control.set_lane_gate(0, Some(true), None).await,
            Err(TransportError::Busy(_))
        ));
        control.record_stop().await;
    }

    #[tokio::test]
    async fn deleting_a_take_reselects_only_when_it_was_the_one_selected() {
        let (control, dir) = shelf().await;
        let project = trib_project::load_latest(dir.path()).unwrap().0;

        // Deleting something else leaves the selection where it is.
        control.delete_take(1).await.unwrap();
        assert_eq!(control.transport().await.take, Some(3));
        assert!(!project.takes_dir().join("take-001").exists());

        // Deleting what we were reviewing falls back to the newest left.
        control.delete_take(3).await.unwrap();
        let dto = control.transport().await;
        assert_eq!(dto.take, Some(2));
        assert_eq!(dto.latest_take, Some(2));

        // And emptying the shelf leaves nothing to play.
        control.delete_take(2).await.unwrap();
        assert_eq!(control.transport().await.take, None);
        assert!(matches!(control.play().await, Err(TransportError::NoTake)));
    }

    /// The feeder threads hold open readers and reopen the files on every
    /// loop pass, so unlinking underneath them would run playback off
    /// unlinked inodes until a later wrap died somewhere confusing.
    #[tokio::test]
    async fn deleting_the_take_being_played_is_refused_and_the_files_survive() {
        let (control, dir) = shelf().await;
        let project = trib_project::load_latest(dir.path()).unwrap().0;
        control.play().await.unwrap();
        assert!(matches!(
            control.delete_take(3).await,
            Err(TransportError::Busy(_))
        ));
        assert!(project.takes_dir().join("take-003").exists());
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
    async fn a_new_session_starts_from_the_chosen_seed() {
        use crate::console::{SessionSeed, fresh_console};
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "first", &state).unwrap();
        let control = spawn_test_in(hub, cmd_tx, state.clone(), params, keys_tx, project);
        control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(0) },
                    level_db: -6.0,
                },
                None,
            )
            .await
            .unwrap();

        let made = control
            .create_session("Clean".into(), SessionSeed::Template)
            .await
            .unwrap();
        assert_eq!(made.name, "Clean");
        assert_eq!(
            control.snapshot().await,
            fresh_console(),
            "the template replaces the desk"
        );
        let dto = control.transport().await;
        assert_eq!(dto.take, None, "a brand new session has an empty shelf");
        assert_eq!(dto.total_frames, 0);

        // Console copies the desk outright.
        let before = control.snapshot().await;
        control
            .create_session("Copy".into(), SessionSeed::Console)
            .await
            .unwrap();
        assert_eq!(control.snapshot().await, before);
    }

    /// The session you left keeps its own console AND its own takes, and
    /// both come back. This is the core promise of the whole feature.
    #[tokio::test]
    async fn switching_away_and_back_restores_the_console_and_the_takes() {
        use crate::console::SessionSeed;
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "first", &state).unwrap();
        fake_take(&project, 1, 48_000);
        let first_id = trib_project::list_sessions(dir.path())[0].id.clone();
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(0) },
                    level_db: -6.0,
                },
                None,
            )
            .await
            .unwrap();
        control
            .create_session("Second".into(), SessionSeed::Template)
            .await
            .unwrap();
        assert_eq!(control.transport().await.take, None, "a fresh shelf");

        control.open_session(first_id).await.unwrap();
        assert_eq!(
            control.snapshot().await.strips[0].fader_db,
            -6.0,
            "the first session's console came back"
        );
        assert_eq!(control.transport().await.take, Some(1), "and its takes");
    }

    #[tokio::test]
    async fn a_session_cannot_be_deleted_while_it_is_open() {
        use crate::console::SessionSeed;
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "first", &state).unwrap();
        let first_id = trib_project::list_sessions(dir.path())[0].id.clone();
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        assert!(matches!(
            control.delete_session(first_id.clone()).await,
            Err(TransportError::Busy(_)),
        ));
        assert!(dir.path().join(&first_id).is_dir(), "still there");

        // Open something else and it becomes deletable.
        control
            .create_session("Second".into(), SessionSeed::Template)
            .await
            .unwrap();
        control.delete_session(first_id.clone()).await.unwrap();
        assert!(!dir.path().join(&first_id).exists());
    }

    #[tokio::test]
    async fn an_unknown_session_id_is_refused_and_nothing_changes() {
        let (control, _dir) = shelf().await;
        let before = control.session().await;
        for bad in ["../escape", "nope", "a/b"] {
            assert!(
                matches!(
                    control.open_session(bad.into()).await,
                    Err(TransportError::NoSuchSession)
                ),
                "accepted {bad:?}"
            );
        }
        assert_eq!(control.session().await, before);
        assert_eq!(control.transport().await.take, Some(3));
    }

    /// A rename writes a name, not audio — and scribbling on the tape
    /// while it rolls is exactly the gesture the label was designed for.
    #[tokio::test]
    async fn a_rename_is_allowed_while_recording_and_lands_in_the_manifest() {
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = armed();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "first", &state).unwrap();
        let id = trib_project::list_sessions(dir.path())[0].id.clone();
        let control = spawn_test_in(hub, cmd_tx, state, params, keys_tx, project);

        control.record_start().await.unwrap();
        let renamed = control
            .rename_session(id.clone(), "Take Two".into())
            .await
            .unwrap();
        assert_eq!(renamed.name, "Take Two");
        assert_eq!(renamed.id, id, "the id is stable across a rename");
        control.record_stop().await;
        assert_eq!(trib_project::list_sessions(dir.path())[0].name, "Take Two");
    }

    #[tokio::test]
    async fn creating_or_switching_is_refused_while_recording() {
        use crate::console::SessionSeed;
        let hub = Hub::new(32);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = armed();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let dir = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(dir.path(), "first", &state).unwrap();
        let id = trib_project::list_sessions(dir.path())[0].id.clone();
        let control = spawn_test_in(hub, cmd_tx, state.clone(), params, keys_tx, project);

        control.record_start().await.unwrap();
        assert!(matches!(
            control
                .create_session("Nope".into(), SessionSeed::Template)
                .await,
            Err(TransportError::Busy(_))
        ));
        assert!(matches!(
            control.open_session(id).await,
            Err(TransportError::Busy(_))
        ));
        assert_eq!(control.snapshot().await, state, "the desk is untouched");
        control.record_stop().await;
    }

    #[tokio::test]
    /// Autosave is debounced by 500 ms and a swap cancels whatever is
    /// pending, so before the pre-swap flush an edit made in the half
    /// second before switching was simply lost: nudge a fader, switch
    /// away, come back, and the nudge is gone. This is the test that pins
    /// the fix, and it fails without it.
    async fn a_console_edit_just_before_a_swap_survives_the_round_trip() {
        let hub = Hub::new(16);
        let (cmd_tx, mut _cmd_rx) = rtrb::RingBuffer::new(64);
        let (state, params, keys) = initial();
        let (keys_tx, _keys_rx) = watch::channel(keys);
        let home = tempfile::tempdir().unwrap();
        let project = trib_project::create_project(home.path(), "first", &state).unwrap();
        let control = spawn_test_in(hub, cmd_tx, state.clone(), params, keys_tx, project);

        // A fader move, then a swap well inside the autosave debounce.
        control
            .apply(
                MixCommand::SetFader {
                    target: FaderTarget::Strip { id: StripId(0) },
                    level_db: -12.0,
                },
                None,
            )
            .await
            .unwrap();
        let away = tempfile::tempdir().unwrap();
        control
            .update_recording(Some(away.path().to_path_buf()), None, None)
            .await
            .unwrap();
        // Back to where we started — this re-adopts the first session's
        // manifest, which is where the edit had to have landed.
        control
            .update_recording(Some(home.path().to_path_buf()), None, None)
            .await
            .unwrap();

        let fader = control.snapshot().await.strips[0].fader_db;
        assert_eq!(fader, -12.0, "the edit was lost in the swap");
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
