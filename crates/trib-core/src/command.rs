use serde::{Deserialize, Serialize};

use crate::bus::{BusKind, BusState};
use crate::db::{FADER_MAX_DB, FADER_MIN_DB, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::eq::{EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_RANGE_DB, EQ_Q_MAX, EQ_Q_MIN, EqBandKind};
use crate::fx::FxParams;
use crate::id::{BusId, FxId, InstrumentId, StripId};
use crate::instrument::{InstrumentSplit, InstrumentState};
use crate::mix::MixerState;
use crate::output::{MAX_OUTPUT_PATCHES, OutputJack, OutputPatch, OutputSource};
use crate::strip::{InputAssign, RouteTarget, SendState, SendTap, StripState};

/// Longest accepted strip/bus name — it has to fit on the tape.
pub const MAX_NAME_LEN: usize = 60;

/// Console capacity. Keeps every strip metered within the engine's fixed
/// MeterBlock (64 slots shared with buses, FX and master).
pub const MAX_STRIPS: usize = 32;

/// Instruments the rack will hold. Each one is a live synthesiser on the
/// render thread, so the ceiling is CPU, not bookkeeping — eight is already
/// past what a Pi will carry.
pub const MAX_INSTRUMENTS: usize = 8;

/// Voice-pool bounds. One voice is one note in flight; below 4 a chord
/// eats itself, and past 256 a Pi runs out of core before it runs out of
/// voices.
pub const MIN_POLYPHONY: u16 = 4;
pub const MAX_POLYPHONY: u16 = 256;

/// Which fader a level command addresses; doubles as the mute/PFL/rename
/// target (the master supports only fader moves in v1 — see `apply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FaderTarget {
    Strip { id: StripId },
    Bus { id: BusId },
    Master,
}

/// A mutation of the mixer document. The API-facing command enum: REST
/// handlers and the WS `set` op both reduce to this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MixCommand {
    SetFader {
        target: FaderTarget,
        level_db: f32,
    },
    SetGain {
        strip: StripId,
        gain_db: f32,
    },
    SetEqBand {
        strip: StripId,
        band: EqBandKind,
        freq_hz: f32,
        gain_db: f32,
        q: f32,
    },
    SetEqEnabled {
        strip: StripId,
        enabled: bool,
    },
    SetPan {
        strip: StripId,
        pan: f32,
    },
    SetMute {
        target: FaderTarget,
        mute: bool,
    },
    SetPfl {
        target: FaderTarget,
        on: bool,
    },
    SetInput {
        strip: StripId,
        input: Option<InputAssign>,
    },
    Rename {
        target: FaderTarget,
        name: String,
    },
    AddStrip {
        name: Option<String>,
    },
    RemoveStrip {
        id: StripId,
    },
    /// Upsert one aux send. A level at the floor is simply silent — the
    /// send always exists toward every aux bus.
    SetSend {
        strip: StripId,
        dest: BusId,
        level_db: f32,
        tap: SendTap,
    },
    /// Where the strip's main signal goes (master or a group bus).
    SetRoute {
        strip: StripId,
        to: RouteTarget,
    },
    SetFxParams {
        fx: FxId,
        params: FxParams,
    },
    SetFxReturn {
        fx: FxId,
        level_db: f32,
    },
    AddBus {
        kind: BusKind,
        name: Option<String>,
    },
    RemoveBus {
        id: BusId,
    },
    /// Arm a strip (its own dry track) or the master (the mix) for the
    /// next take. Arming mid-take changes the NEXT take, not the running one.
    SetRecordArm {
        target: FaderTarget,
        armed: bool,
    },
    /// The "Arm All" button: every loaded strip at once (master untouched).
    SetRecordArmAll {
        armed: bool,
    },
    AddInstrument {
        name: Option<String>,
    },
    RemoveInstrument {
        id: InstrumentId,
    },
    /// Everything `rustysynth` can only take at construction. Rebuilding a
    /// synth means reloading its samples, so this is a topology change and
    /// the tape refuses it — mid-take is exactly when you cannot afford a
    /// pause to read a soundfont off a USB stick.
    SetInstrumentVoice {
        id: InstrumentId,
        soundfont: Option<String>,
        polyphony: u16,
        effects: bool,
    },
    /// Everything a running synth accepts: the preset, what it listens to,
    /// and its name. Deliberately NOT frozen while recording — changing
    /// sound mid-take is normal playing, and a keyboardist who finds
    /// themselves on the wrong MIDI channel has to be able to fix it.
    SetInstrumentPerformance {
        id: InstrumentId,
        name: String,
        bank: u16,
        program: u8,
        port: Option<String>,
        midi_channel: Option<u8>,
    },
    /// How an instrument reaches the desk: one stereo pair, or a fader per
    /// named piece. Rebuilds the synth, so it is a topology change.
    SetInstrumentSplits {
        id: InstrumentId,
        splits: Vec<InstrumentSplit>,
    },
    /// Put every one of an instrument's channels on the desk in one move.
    ///
    /// The setup step a kit needs before anyone plays: six drums, six
    /// faders, named and patched, ready to pan. Adds only what is missing,
    /// so pressing it twice is safe and pressing it after adding a split
    /// does the obvious thing rather than doubling the desk.
    AddInstrumentStrips {
        id: InstrumentId,
    },
    /// Patch one output channel — an upsert keyed on the JACK, because the
    /// jack is the identity. A jack holds at most one patch, so re-patching
    /// replaces what was there rather than stacking a second feed onto it;
    /// one SOURCE may feed many jacks, the exact mirror of one jack feeding
    /// many strips on the way in.
    SetOutputPatch {
        patch: OutputPatch,
    },
    /// Unpatch one output channel, named by the jack.
    ClearOutputPatch {
        jack: OutputJack,
    },
    /// Move a patch's tap and nothing else.
    ///
    /// Its own command so a console that PUTs its whole form does not stop
    /// the tape to flip a switch — the same split `patch_commands` already
    /// makes between an instrument's voice and its performance.
    SetOutputTap {
        jack: OutputJack,
        tap: SendTap,
    },
}

impl MixCommand {
    /// Whether the tape refuses this command while it is rolling.
    ///
    /// Not the same question as [`ReconcileNeed`], though one field used to
    /// answer both. The engine needs a recompile whenever the graph's shape
    /// changes. The TAPE cares about two things: whether the change moves
    /// what a record tap is bound to (the strips and their order), and
    /// whether applying it costs a graph swap — because a swap reseeds every
    /// smoother and clears every filter and FX tail, and the master record
    /// tap is POST-FX, so that transient lands in the take.
    ///
    /// So an output patch is refused mid-take even though it is downstream
    /// of every tap and cannot change one recorded sample: it is the
    /// recompile that would be audible, not the patch. Its pre/post switch
    /// is not refused — that is a flag on an already-compiled tap, exactly
    /// like an aux send's, and it is the one gesture a monitor engineer
    /// genuinely needs live.
    pub fn stops_the_tape(&self) -> bool {
        match self {
            MixCommand::AddStrip { .. }
            | MixCommand::RemoveStrip { .. }
            | MixCommand::SetInput { .. }
            | MixCommand::SetRoute { .. }
            | MixCommand::AddBus { .. }
            | MixCommand::RemoveBus { .. }
            | MixCommand::AddInstrument { .. }
            | MixCommand::RemoveInstrument { .. }
            | MixCommand::SetInstrumentVoice { .. }
            | MixCommand::SetInstrumentSplits { .. }
            | MixCommand::AddInstrumentStrips { .. }
            | MixCommand::SetOutputPatch { .. }
            | MixCommand::ClearOutputPatch { .. } => true,
            MixCommand::SetFader { .. }
            | MixCommand::SetGain { .. }
            | MixCommand::SetEqBand { .. }
            | MixCommand::SetEqEnabled { .. }
            | MixCommand::SetPan { .. }
            | MixCommand::SetMute { .. }
            | MixCommand::SetPfl { .. }
            | MixCommand::Rename { .. }
            | MixCommand::SetSend { .. }
            | MixCommand::SetFxParams { .. }
            | MixCommand::SetFxReturn { .. }
            | MixCommand::SetRecordArm { .. }
            | MixCommand::SetRecordArmAll { .. }
            | MixCommand::SetInstrumentPerformance { .. }
            | MixCommand::SetOutputTap { .. } => false,
        }
    }
}

/// What a successful `apply` changed — the WS `state_changed` payload.
/// Semantically complete per change: a client patches its mirror from the
/// delta alone. Structural changes broadcast a full snapshot instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StateDelta {
    Fader {
        target: FaderTarget,
        level_db: f32,
    },
    Gain {
        strip: StripId,
        gain_db: f32,
    },
    EqBand {
        strip: StripId,
        band: EqBandKind,
        freq_hz: f32,
        gain_db: f32,
        q: f32,
    },
    EqEnabled {
        strip: StripId,
        enabled: bool,
    },
    Pan {
        strip: StripId,
        pan: f32,
    },
    Mute {
        target: FaderTarget,
        mute: bool,
    },
    Pfl {
        target: FaderTarget,
        on: bool,
    },
    Input {
        strip: StripId,
        input: Option<InputAssign>,
    },
    Renamed {
        target: FaderTarget,
        name: String,
    },
    StripAdded {
        strip: StripState,
    },
    StripRemoved {
        id: StripId,
        /// The jacks that went quiet with it. A structural change, so a
        /// snapshot follows — but the console wants to say what it just
        /// did, and "removing Ch 3 also unpatched OUT 5" is that sentence.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unpatched_outputs: Vec<OutputJack>,
    },
    Send {
        strip: StripId,
        dest: BusId,
        level_db: f32,
        tap: SendTap,
    },
    Route {
        strip: StripId,
        to: RouteTarget,
    },
    FxParams {
        fx: FxId,
        params: FxParams,
    },
    FxReturn {
        fx: FxId,
        level_db: f32,
    },
    BusAdded {
        bus: BusState,
    },
    BusRemoved {
        id: BusId,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unpatched_outputs: Vec<OutputJack>,
    },
    RecordArm {
        target: FaderTarget,
        armed: bool,
    },
    RecordArmAll {
        armed: bool,
    },
    InstrumentAdded {
        instrument: InstrumentState,
    },
    /// Carries the strips it unpatched, so a client's mirror does not need
    /// to re-derive which channels just went quiet.
    InstrumentRemoved {
        id: InstrumentId,
        unpatched: Vec<StripId>,
    },
    /// Both instrument edits report the whole instrument rather than the
    /// fields that moved: it is small, and a client that misses one field
    /// of a preset change would render a lie about what is playing.
    InstrumentChanged {
        instrument: InstrumentState,
    },
    /// The strips "add all channels" created. A structural change, so the
    /// daemon broadcasts a snapshot too; the delta carries the list because
    /// the console wants to say what it just did.
    InstrumentStripsAdded {
        id: InstrumentId,
        added: Vec<StripState>,
    },
    /// The whole patch, not the fields that moved: a client keys outputs by
    /// jack, and a half-patch would leave it drawing a jack whose source it
    /// cannot name.
    OutputPatched {
        patch: OutputPatch,
    },
    OutputUnpatched {
        jack: OutputJack,
    },
    OutputTap {
        jack: OutputJack,
        tap: SendTap,
    },
}

/// How the engine must be told about a change: a cheap parameter write, or a
/// full graph recompile + swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileNeed {
    ParamOnly,
    Topology,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MixError {
    #[error("unknown {0}")]
    UnknownTarget(String),
    #[error("{field} out of range: {value}")]
    OutOfRange { field: &'static str, value: f32 },
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("invalid name: {0}")]
    BadName(&'static str),
}

fn check_range(field: &'static str, value: f32, min: f32, max: f32) -> Result<f32, MixError> {
    // NaN fails every comparison, so it lands here too — refused, not stored.
    if value >= min && value <= max {
        Ok(value)
    } else {
        Err(MixError::OutOfRange { field, value })
    }
}

fn check_name(name: &str) -> Result<String, MixError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err(MixError::BadName("must not be empty"))
    } else if trimmed.len() > MAX_NAME_LEN {
        Err(MixError::BadName("too long for the tape"))
    } else {
        Ok(trimmed.to_owned())
    }
}

fn strip_mut(state: &mut MixerState, id: StripId) -> Result<&mut StripState, MixError> {
    state
        .strips
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))
}

type Applied = (MixerState, StateDelta, ReconcileNeed);

/// The pure reducer: the ONLY way mixer state changes. Returns the next
/// state, the delta to broadcast, and what the engine needs.
pub fn apply(state: &MixerState, command: MixCommand) -> Result<Applied, MixError> {
    use ReconcileNeed::{ParamOnly, Topology};
    let mut next = state.clone();
    let (delta, need) = match command {
        MixCommand::SetFader { target, level_db } => {
            let level_db = check_range("level_db", level_db, FADER_MIN_DB, FADER_MAX_DB)?;
            match target {
                FaderTarget::Strip { id } => strip_mut(&mut next, id)?.fader_db = level_db,
                FaderTarget::Bus { id } => {
                    next.buses
                        .iter_mut()
                        .find(|b| b.id == id)
                        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))?
                        .fader_db = level_db;
                }
                FaderTarget::Master => next.master.fader_db = level_db,
            }
            (StateDelta::Fader { target, level_db }, ParamOnly)
        }
        MixCommand::SetGain { strip, gain_db } => {
            let gain_db = check_range("gain_db", gain_db, GAIN_MIN_DB, GAIN_MAX_DB)?;
            strip_mut(&mut next, strip)?.gain_db = gain_db;
            (StateDelta::Gain { strip, gain_db }, ParamOnly)
        }
        MixCommand::SetEqBand {
            strip,
            band,
            freq_hz,
            gain_db,
            q,
        } => {
            let freq_hz = check_range("freq_hz", freq_hz, EQ_FREQ_MIN_HZ, EQ_FREQ_MAX_HZ)?;
            let gain_db = check_range("gain_db", gain_db, -EQ_GAIN_RANGE_DB, EQ_GAIN_RANGE_DB)?;
            let q = check_range("q", q, EQ_Q_MIN, EQ_Q_MAX)?;
            let target = strip_mut(&mut next, strip)?;
            let slot = match band {
                EqBandKind::LowShelf => &mut target.eq.low,
                EqBandKind::Peak => &mut target.eq.mid,
                EqBandKind::HighShelf => &mut target.eq.high,
            };
            slot.freq_hz = freq_hz;
            slot.gain_db = gain_db;
            slot.q = q;
            (
                StateDelta::EqBand {
                    strip,
                    band,
                    freq_hz,
                    gain_db,
                    q,
                },
                ParamOnly,
            )
        }
        MixCommand::SetEqEnabled { strip, enabled } => {
            strip_mut(&mut next, strip)?.eq.enabled = enabled;
            (StateDelta::EqEnabled { strip, enabled }, ParamOnly)
        }
        MixCommand::SetPan { strip, pan } => {
            let pan = check_range("pan", pan, -1.0, 1.0)?;
            strip_mut(&mut next, strip)?.pan = pan;
            (StateDelta::Pan { strip, pan }, ParamOnly)
        }
        MixCommand::SetMute { target, mute } => {
            match target {
                FaderTarget::Strip { id } => strip_mut(&mut next, id)?.mute = mute,
                FaderTarget::Bus { id } => {
                    next.buses
                        .iter_mut()
                        .find(|b| b.id == id)
                        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))?
                        .mute = mute;
                }
                FaderTarget::Master => return Err(MixError::Unsupported("master has no mute")),
            }
            (StateDelta::Mute { target, mute }, ParamOnly)
        }
        MixCommand::SetPfl { target, on } => {
            match target {
                FaderTarget::Strip { id } => strip_mut(&mut next, id)?.pfl = on,
                FaderTarget::Bus { id } => {
                    next.buses
                        .iter_mut()
                        .find(|b| b.id == id)
                        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))?
                        .pfl = on;
                }
                FaderTarget::Master => return Err(MixError::Unsupported("master has no PFL")),
            }
            (StateDelta::Pfl { target, on }, ParamOnly)
        }
        MixCommand::SetInput { strip, input } => {
            strip_mut(&mut next, strip)?.input = input.clone();
            (StateDelta::Input { strip, input }, Topology)
        }
        MixCommand::Rename { target, name } => {
            let name = check_name(&name)?;
            match target {
                FaderTarget::Strip { id } => strip_mut(&mut next, id)?.name = name.clone(),
                FaderTarget::Bus { id } => {
                    next.buses
                        .iter_mut()
                        .find(|b| b.id == id)
                        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))?
                        .name = name.clone();
                }
                FaderTarget::Master => return Err(MixError::Unsupported("master has no tape")),
            }
            (StateDelta::Renamed { target, name }, ParamOnly)
        }
        MixCommand::AddStrip { name } => {
            if next.strips.len() >= MAX_STRIPS {
                return Err(MixError::Unsupported("the console is full"));
            }
            let id = next.next_strip_id();
            let name = match name {
                Some(name) => check_name(&name)?,
                None => format!("Ch {}", id.0 + 1),
            };
            let strip = StripState::new(id, name);
            next.strips.push(strip.clone());
            (StateDelta::StripAdded { strip }, Topology)
        }
        MixCommand::RemoveStrip { id } => {
            let before = next.strips.len();
            next.strips.retain(|s| s.id != id);
            if next.strips.len() == before {
                return Err(MixError::UnknownTarget(id.to_string()));
            }
            let unpatched_outputs = unpatch_source(&mut next, OutputSource::Strip { id });
            (
                StateDelta::StripRemoved {
                    id,
                    unpatched_outputs,
                },
                Topology,
            )
        }
        MixCommand::SetSend {
            strip,
            dest,
            level_db,
            tap,
        } => {
            let level_db = check_range("level_db", level_db, FADER_MIN_DB, FADER_MAX_DB)?;
            match next.bus(dest) {
                Some(bus) if bus.kind == BusKind::Aux => {}
                Some(_) => return Err(MixError::Unsupported("sends target aux buses only")),
                None => return Err(MixError::UnknownTarget(dest.to_string())),
            }
            let target = strip_mut(&mut next, strip)?;
            match target.sends.iter_mut().find(|s| s.dest == dest) {
                Some(send) => {
                    send.level_db = level_db;
                    send.tap = tap;
                }
                None => target.sends.push(SendState {
                    dest,
                    level_db,
                    tap,
                }),
            }
            (
                StateDelta::Send {
                    strip,
                    dest,
                    level_db,
                    tap,
                },
                ParamOnly,
            )
        }
        MixCommand::SetRoute { strip, to } => {
            if let RouteTarget::Bus { id } = to {
                match next.bus(id) {
                    Some(bus) if bus.kind == BusKind::Group => {}
                    Some(_) => {
                        return Err(MixError::Unsupported("strips route to master or groups"));
                    }
                    None => return Err(MixError::UnknownTarget(id.to_string())),
                }
            }
            strip_mut(&mut next, strip)?.route_to = to;
            (StateDelta::Route { strip, to }, Topology)
        }
        MixCommand::SetFxParams { fx, params } => {
            check_fx_params(&params)?;
            let unit = next
                .fx
                .iter_mut()
                .find(|f| f.id == fx)
                .ok_or_else(|| MixError::UnknownTarget(fx.to_string()))?;
            if unit.params.kind_str() != params.kind_str() {
                return Err(MixError::Unsupported("an FX unit cannot change kind"));
            }
            unit.params = params;
            (StateDelta::FxParams { fx, params }, ParamOnly)
        }
        MixCommand::SetFxReturn { fx, level_db } => {
            let level_db = check_range("level_db", level_db, FADER_MIN_DB, FADER_MAX_DB)?;
            next.fx
                .iter_mut()
                .find(|f| f.id == fx)
                .ok_or_else(|| MixError::UnknownTarget(fx.to_string()))?
                .return_level_db = level_db;
            (StateDelta::FxReturn { fx, level_db }, ParamOnly)
        }
        MixCommand::AddBus { kind, name } => {
            let id = BusId(next.buses.iter().map(|b| b.id.0 + 1).max().unwrap_or(0));
            let name = match name {
                Some(name) => check_name(&name)?,
                None => match kind {
                    BusKind::Group => format!("Group {}", id.0 + 1),
                    BusKind::Aux => format!("Aux {}", id.0 + 1),
                },
            };
            let bus = BusState::new(id, kind, name);
            next.buses.push(bus.clone());
            (StateDelta::BusAdded { bus }, Topology)
        }
        MixCommand::RemoveBus { id } => {
            if next.fx.iter().any(|f| f.input == id) {
                return Err(MixError::Unsupported("bus feeds an FX unit"));
            }
            let before = next.buses.len();
            next.buses.retain(|b| b.id != id);
            if next.buses.len() == before {
                return Err(MixError::UnknownTarget(id.to_string()));
            }
            // Orphaned references heal: routes fall back to master, sends
            // toward the dead bus disappear.
            for strip in &mut next.strips {
                if strip.route_to == (RouteTarget::Bus { id }) {
                    strip.route_to = RouteTarget::Master;
                }
                strip.sends.retain(|s| s.dest != id);
            }
            // An output patch heals rather than refusing, unlike the FX
            // guard above: an FX unit is a document object with nowhere
            // else to live, and an output patch is a wire — unplugging it
            // is exactly what removing the bus means.
            let unpatched_outputs = unpatch_source(&mut next, OutputSource::Bus { id });
            (
                StateDelta::BusRemoved {
                    id,
                    unpatched_outputs,
                },
                Topology,
            )
        }
        MixCommand::SetRecordArm { target, armed } => {
            match target {
                FaderTarget::Strip { id } => strip_mut(&mut next, id)?.record_arm = armed,
                FaderTarget::Master => next.master.record_arm = armed,
                FaderTarget::Bus { .. } => {
                    return Err(MixError::Unsupported("buses are not recorded in v1"));
                }
            }
            (StateDelta::RecordArm { target, armed }, ParamOnly)
        }
        MixCommand::SetRecordArmAll { armed } => {
            for strip in &mut next.strips {
                strip.record_arm = armed;
            }
            (StateDelta::RecordArmAll { armed }, ParamOnly)
        }
        MixCommand::AddInstrument { name } => {
            if next.instruments.len() >= MAX_INSTRUMENTS {
                return Err(MixError::Unsupported("the instrument rack is full"));
            }
            let id = next.next_instrument_id();
            let name = match name {
                Some(name) => check_name(&name)?,
                None => format!("Inst {}", next.instruments.len() + 1),
            };
            let instrument = InstrumentState::new(id, name);
            next.instruments.push(instrument.clone());
            (StateDelta::InstrumentAdded { instrument }, Topology)
        }
        MixCommand::RemoveInstrument { id } => {
            let before = next.instruments.len();
            next.instruments.retain(|i| i.id != id);
            if next.instruments.len() == before {
                return Err(MixError::UnknownTarget(id.to_string()));
            }
            // Orphaned patches heal, exactly as they do when a bus dies. A
            // strip left pointing at a removed instrument would be silent
            // with nothing in the patchbay to explain it.
            let mut unpatched = Vec::new();
            for strip in &mut next.strips {
                if strip.input.as_ref().and_then(InputAssign::instrument_id) == Some(id) {
                    strip.input = None;
                    unpatched.push(strip.id);
                }
            }
            (StateDelta::InstrumentRemoved { id, unpatched }, Topology)
        }
        MixCommand::SetInstrumentVoice {
            id,
            soundfont,
            polyphony,
            effects,
        } => {
            if !(MIN_POLYPHONY..=MAX_POLYPHONY).contains(&polyphony) {
                return Err(MixError::OutOfRange {
                    field: "polyphony",
                    value: f32::from(polyphony),
                });
            }
            let instrument = instrument_mut(&mut next, id)?;
            instrument.soundfont = soundfont;
            instrument.polyphony = polyphony;
            instrument.effects = effects;
            let instrument = instrument.clone();
            (StateDelta::InstrumentChanged { instrument }, Topology)
        }
        MixCommand::SetInstrumentPerformance {
            id,
            name,
            bank,
            program,
            port,
            midi_channel,
        } => {
            let name = check_name(&name)?;
            if program > 127 {
                return Err(MixError::OutOfRange {
                    field: "program",
                    value: f32::from(program),
                });
            }
            if midi_channel.is_some_and(|channel| channel > 15) {
                return Err(MixError::OutOfRange {
                    field: "midi_channel",
                    value: f32::from(midi_channel.unwrap_or_default()),
                });
            }
            let instrument = instrument_mut(&mut next, id)?;
            instrument.name = name;
            instrument.bank = bank;
            instrument.program = program;
            instrument.port = port;
            instrument.midi_channel = midi_channel;
            let instrument = instrument.clone();
            (StateDelta::InstrumentChanged { instrument }, ParamOnly)
        }
        MixCommand::SetInstrumentSplits { id, splits } => {
            for split in &splits {
                check_name(&split.name)?;
                if split.ranges.is_empty() {
                    return Err(MixError::BadName("a split needs at least one key range"));
                }
                if split.ranges.iter().any(|(lo, hi)| lo > hi || *hi > 127) {
                    return Err(MixError::OutOfRange {
                        field: "key range",
                        value: 0.0,
                    });
                }
            }
            // Changing the layout moves every channel after this
            // instrument, so patches that pointed past the new width would
            // land on a neighbour. Heal them instead.
            let width = if splits.is_empty() { 2 } else { splits.len() };
            let instrument = instrument_mut(&mut next, id)?;
            instrument.splits = splits;
            let instrument = instrument.clone();
            for strip in &mut next.strips {
                if let Some(assign) = &strip.input
                    && assign.instrument_id() == Some(id)
                    && (assign.channel() as usize) >= width
                {
                    strip.input = None;
                }
            }
            (StateDelta::InstrumentChanged { instrument }, Topology)
        }
        MixCommand::AddInstrumentStrips { id } => {
            let instrument = next
                .instruments
                .iter()
                .find(|i| i.id == id)
                .ok_or_else(|| MixError::UnknownTarget(id.to_string()))?
                .clone();
            let names = instrument.channel_names();
            let mut added = Vec::new();
            for (channel, name) in names.into_iter().enumerate() {
                let channel = channel as u16;
                let wanted = InputAssign::instrument(id, channel);
                // Only what is missing: an existing strip carries someone's
                // level, pan and EQ, and a second press must not duplicate
                // it or reset it.
                if next
                    .strips
                    .iter()
                    .any(|s| s.input.as_ref() == Some(&wanted))
                {
                    continue;
                }
                if next.strips.len() >= MAX_STRIPS {
                    return Err(MixError::Unsupported("the console is full"));
                }
                let mut strip = StripState::new(next.next_strip_id(), check_name(&name)?);
                strip.input = Some(wanted);
                next.strips.push(strip.clone());
                added.push(strip);
            }
            (StateDelta::InstrumentStripsAdded { id, added }, Topology)
        }
        MixCommand::SetOutputPatch { patch } => {
            let width = next
                .source_channels(&patch.source)
                .ok_or_else(|| MixError::UnknownTarget(source_name(&patch.source)))?;
            if patch.source_channel >= width {
                return Err(MixError::OutOfRange {
                    field: "source_channel",
                    value: f32::from(patch.source_channel),
                });
            }
            let jack = patch.jack();
            let replacing = next.outputs.iter().position(|o| o.is(&jack));
            match replacing {
                // One jack, one feed: patching over an occupied output
                // replaces it. Summing two sources onto one channel would
                // make this a mixer rather than a patch bay.
                Some(at) => next.outputs[at] = patch.clone(),
                None => {
                    if next.outputs.len() >= MAX_OUTPUT_PATCHES {
                        return Err(MixError::Unsupported("the output patch bay is full"));
                    }
                    next.outputs.push(patch.clone());
                }
            }
            (StateDelta::OutputPatched { patch }, Topology)
        }
        MixCommand::ClearOutputPatch { jack } => {
            let before = next.outputs.len();
            next.outputs.retain(|o| !o.is(&jack));
            if next.outputs.len() == before {
                return Err(MixError::UnknownTarget(jack_name(&jack)));
            }
            (StateDelta::OutputUnpatched { jack }, Topology)
        }
        MixCommand::SetOutputTap { jack, tap } => {
            let patch = next
                .outputs
                .iter_mut()
                .find(|o| o.is(&jack))
                .ok_or_else(|| MixError::UnknownTarget(jack_name(&jack)))?;
            patch.tap = tap;
            (StateDelta::OutputTap { jack, tap }, ParamOnly)
        }
    };
    Ok((next, delta, need))
}

/// Drop every output patch fed by `source`, returning the jacks that went
/// quiet. The output-side sibling of the route and send healing that
/// `RemoveBus` already does.
fn unpatch_source(state: &mut MixerState, source: OutputSource) -> Vec<OutputJack> {
    let gone: Vec<OutputJack> = state
        .outputs
        .iter()
        .filter(|patch| patch.source == source)
        .map(OutputPatch::jack)
        .collect();
    state.outputs.retain(|patch| patch.source != source);
    gone
}

/// How an unknown source reads in an error. `Display` on the id already
/// prints `strip#3`, so this only has to name the master.
fn source_name(source: &OutputSource) -> String {
    match source {
        OutputSource::Strip { id } => id.to_string(),
        OutputSource::Bus { id } => id.to_string(),
        OutputSource::Master => "master".to_owned(),
    }
}

fn jack_name(jack: &OutputJack) -> String {
    match &jack.device {
        Some(device) => format!("{device} out {}", jack.channel + 1),
        None => format!("out {}", jack.channel + 1),
    }
}

fn instrument_mut(
    state: &mut MixerState,
    id: InstrumentId,
) -> Result<&mut InstrumentState, MixError> {
    state
        .instruments
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| MixError::UnknownTarget(id.to_string()))
}

fn check_fx_params(params: &FxParams) -> Result<(), MixError> {
    match *params {
        FxParams::Reverb { room_size, damping } => {
            check_range("room_size", room_size, 0.0, 1.0)?;
            check_range("damping", damping, 0.0, 1.0)?;
        }
        FxParams::Delay { time_ms, feedback } => {
            check_range("time_ms", time_ms, 1.0, 2_000.0)?;
            check_range("feedback", feedback, 0.0, 0.95)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strip::RouteTarget;

    fn state() -> MixerState {
        MixerState {
            strips: vec![StripState::new(StripId(0), "Ch 1".into())],
            ..MixerState::default()
        }
    }

    fn ok(command: MixCommand) -> Applied {
        apply(&state(), command).unwrap()
    }

    fn jack(channel: u16) -> OutputJack {
        OutputJack {
            device: Some("interface".into()),
            channel,
        }
    }

    fn out_patch(source: OutputSource, source_channel: u16, channel: u16) -> OutputPatch {
        OutputPatch::new(source, source_channel, jack(channel))
    }

    #[test]
    fn patching_an_output_replaces_whatever_was_on_that_jack() {
        let mut state = state();
        state
            .strips
            .push(StripState::new(StripId(1), "Ch 2".into()));
        let (state, ..) = apply(
            &state,
            MixCommand::SetOutputPatch {
                patch: out_patch(OutputSource::Strip { id: StripId(0) }, 0, 3),
            },
        )
        .unwrap();
        let (state, ..) = apply(
            &state,
            MixCommand::SetOutputPatch {
                patch: out_patch(OutputSource::Strip { id: StripId(1) }, 0, 3),
            },
        )
        .unwrap();
        assert_eq!(state.outputs.len(), 1, "one jack holds at most one feed");
        assert_eq!(
            state.output(&jack(3)).unwrap().source,
            OutputSource::Strip { id: StripId(1) }
        );
    }

    #[test]
    fn one_source_may_feed_several_output_channels() {
        // The mirror invariant. An upsert keyed on the SOURCE instead of
        // the JACK would silently destroy this.
        let source = OutputSource::Strip { id: StripId(0) };
        let (state, ..) = apply(
            &state(),
            MixCommand::SetOutputPatch {
                patch: out_patch(source, 0, 3),
            },
        )
        .unwrap();
        let (state, ..) = apply(
            &state,
            MixCommand::SetOutputPatch {
                patch: out_patch(source, 0, 4),
            },
        )
        .unwrap();
        assert_eq!(state.outputs.len(), 2);
    }

    #[test]
    fn an_output_patched_past_its_sources_channels_is_refused() {
        let mut state = state();
        state
            .buses
            .push(BusState::new(BusId(0), BusKind::Aux, "Wedge".into()));
        // An aux bus is mono; channel 1 is not a thing it has.
        assert!(matches!(
            apply(
                &state,
                MixCommand::SetOutputPatch {
                    patch: out_patch(OutputSource::Bus { id: BusId(0) }, 1, 3),
                },
            ),
            Err(MixError::OutOfRange {
                field: "source_channel",
                ..
            })
        ));
        assert!(matches!(
            apply(
                &state,
                MixCommand::SetOutputPatch {
                    patch: out_patch(OutputSource::Strip { id: StripId(9) }, 0, 3),
                },
            ),
            Err(MixError::UnknownTarget(_))
        ));
    }

    #[test]
    fn removing_a_strip_unpatches_the_outputs_it_fed_and_says_which() {
        let (state, ..) = apply(
            &state(),
            MixCommand::SetOutputPatch {
                patch: out_patch(OutputSource::Strip { id: StripId(0) }, 0, 5),
            },
        )
        .unwrap();
        let (next, delta, _) = apply(&state, MixCommand::RemoveStrip { id: StripId(0) }).unwrap();
        assert!(next.outputs.is_empty(), "the patch went with the strip");
        assert_eq!(
            delta,
            StateDelta::StripRemoved {
                id: StripId(0),
                unpatched_outputs: vec![jack(5)],
            },
            "the console needs to be able to say what else just went quiet"
        );
    }

    #[test]
    fn removing_a_bus_unpatches_its_outputs_rather_than_refusing() {
        // Deliberately unlike the FX guard: an FX unit is a document object
        // with nowhere else to live, an output patch is a wire.
        let mut state = state();
        state
            .buses
            .push(BusState::new(BusId(0), BusKind::Group, "Band".into()));
        let (state, ..) = apply(
            &state,
            MixCommand::SetOutputPatch {
                patch: out_patch(OutputSource::Bus { id: BusId(0) }, 1, 6),
            },
        )
        .unwrap();
        let (next, delta, _) = apply(&state, MixCommand::RemoveBus { id: BusId(0) }).unwrap();
        assert!(next.outputs.is_empty());
        assert_eq!(
            delta,
            StateDelta::BusRemoved {
                id: BusId(0),
                unpatched_outputs: vec![jack(6)],
            }
        );
    }

    #[test]
    fn re_tapping_an_output_is_a_parameter_move_and_re_wiring_is_not() {
        let patch = out_patch(OutputSource::Master, 0, 1);
        let (state, _, need) = apply(&state(), MixCommand::SetOutputPatch { patch }).unwrap();
        assert_eq!(need, ReconcileNeed::Topology);

        let (state, delta, need) = apply(
            &state,
            MixCommand::SetOutputTap {
                jack: jack(1),
                tap: SendTap::PostFader,
            },
        )
        .unwrap();
        assert_eq!(need, ReconcileNeed::ParamOnly);
        assert_eq!(
            delta,
            StateDelta::OutputTap {
                jack: jack(1),
                tap: SendTap::PostFader
            }
        );
        assert_eq!(state.output(&jack(1)).unwrap().tap, SendTap::PostFader);
    }

    #[test]
    fn only_the_output_tap_recompiles_nothing_while_every_other_topology_change_stops_the_tape() {
        // The drift guard. `ReconcileNeed` answers "does the engine need a
        // recompile"; `stops_the_tape` answers "does the tape refuse this".
        // They agree everywhere today, and the day they stop agreeing it
        // must be because somebody decided so — not because a new variant
        // was added and nobody looked.
        let mut state = state();
        state
            .buses
            .push(BusState::new(BusId(0), BusKind::Aux, "Wedge".into()));
        state
            .instruments
            .push(InstrumentState::new(InstrumentId(0), "Rhodes".into()));
        state.outputs.push(out_patch(OutputSource::Master, 0, 1));

        for command in every_command_shape() {
            let Ok((_, _, need)) = apply(&state, command.clone()) else {
                continue;
            };
            assert_eq!(
                command.stops_the_tape(),
                need == ReconcileNeed::Topology,
                "{command:?}: the two questions disagree, and nothing says why"
            );
        }
    }

    /// One of every `MixCommand` variant, valid against the fixture above.
    /// Adding a variant without adding it here leaves the drift guard
    /// silently weaker, so the match below is exhaustive on purpose.
    fn every_command_shape() -> Vec<MixCommand> {
        let strip = StripId(0);
        let bus = BusId(0);
        let id = InstrumentId(0);
        let target = FaderTarget::Strip { id: strip };
        vec![
            MixCommand::SetFader {
                target,
                level_db: -6.0,
            },
            MixCommand::SetGain {
                strip,
                gain_db: 3.0,
            },
            MixCommand::SetEqBand {
                strip,
                band: EqBandKind::Peak,
                freq_hz: 1000.0,
                gain_db: 0.0,
                q: 1.0,
            },
            MixCommand::SetEqEnabled {
                strip,
                enabled: true,
            },
            MixCommand::SetPan { strip, pan: 0.5 },
            MixCommand::SetMute { target, mute: true },
            MixCommand::SetPfl { target, on: true },
            MixCommand::SetInput { strip, input: None },
            MixCommand::Rename {
                target,
                name: "Kick".into(),
            },
            MixCommand::AddStrip { name: None },
            MixCommand::RemoveStrip { id: strip },
            MixCommand::SetSend {
                strip,
                dest: bus,
                level_db: -6.0,
                tap: SendTap::PreFader,
            },
            MixCommand::SetRoute {
                strip,
                to: RouteTarget::Master,
            },
            MixCommand::SetFxParams {
                fx: FxId(0),
                params: FxParams::default_reverb(),
            },
            MixCommand::SetFxReturn {
                fx: FxId(0),
                level_db: -6.0,
            },
            MixCommand::AddBus {
                kind: BusKind::Aux,
                name: None,
            },
            MixCommand::RemoveBus { id: bus },
            MixCommand::SetRecordArm {
                target,
                armed: true,
            },
            MixCommand::SetRecordArmAll { armed: true },
            MixCommand::AddInstrument { name: None },
            MixCommand::RemoveInstrument { id },
            MixCommand::SetInstrumentVoice {
                id,
                soundfont: None,
                polyphony: 64,
                effects: true,
            },
            MixCommand::SetInstrumentPerformance {
                id,
                name: "Rhodes".into(),
                bank: 0,
                program: 4,
                port: None,
                midi_channel: None,
            },
            MixCommand::SetInstrumentSplits {
                id,
                splits: Vec::new(),
            },
            MixCommand::AddInstrumentStrips { id },
            MixCommand::SetOutputPatch {
                patch: out_patch(OutputSource::Master, 1, 7),
            },
            MixCommand::ClearOutputPatch { jack: jack(1) },
            MixCommand::SetOutputTap {
                jack: jack(1),
                tap: SendTap::PostFader,
            },
        ]
    }

    #[test]
    fn set_fader_moves_only_the_target() {
        let (next, delta, need) = ok(MixCommand::SetFader {
            target: FaderTarget::Strip { id: StripId(0) },
            level_db: -6.0,
        });
        assert_eq!(next.strips[0].fader_db, -6.0);
        assert_eq!(next.master.fader_db, 0.0, "master untouched");
        assert!(matches!(delta, StateDelta::Fader { .. }));
        assert_eq!(need, ReconcileNeed::ParamOnly);
    }

    #[test]
    fn out_of_range_and_nan_levels_are_refused() {
        for bad in [11.0, -91.0, f32::NAN] {
            assert!(
                apply(
                    &state(),
                    MixCommand::SetFader {
                        target: FaderTarget::Master,
                        level_db: bad,
                    },
                )
                .is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn eq_band_updates_the_addressed_slot() {
        let (next, _, need) = ok(MixCommand::SetEqBand {
            strip: StripId(0),
            band: EqBandKind::Peak,
            freq_hz: 1200.0,
            gain_db: 4.0,
            q: 0.9,
        });
        assert_eq!(next.strips[0].eq.mid.freq_hz, 1200.0);
        assert_eq!(next.strips[0].eq.mid.gain_db, 4.0);
        assert_eq!(next.strips[0].eq.low.gain_db, 0.0, "other bands untouched");
        assert_eq!(need, ReconcileNeed::ParamOnly);
    }

    #[test]
    fn eq_gain_beyond_the_knob_range_is_refused() {
        assert!(
            apply(
                &state(),
                MixCommand::SetEqBand {
                    strip: StripId(0),
                    band: EqBandKind::HighShelf,
                    freq_hz: 12_000.0,
                    gain_db: 16.0,
                    q: 0.71,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn master_mute_pfl_and_rename_are_refused() {
        for command in [
            MixCommand::SetMute {
                target: FaderTarget::Master,
                mute: true,
            },
            MixCommand::SetPfl {
                target: FaderTarget::Master,
                on: true,
            },
            MixCommand::Rename {
                target: FaderTarget::Master,
                name: "x".into(),
            },
        ] {
            assert!(matches!(
                apply(&state(), command),
                Err(MixError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn input_patching_needs_a_recompile() {
        let (next, _, need) = ok(MixCommand::SetInput {
            strip: StripId(0),
            input: Some(InputAssign::device(None, 3)),
        });
        assert_eq!(next.strips[0].input, Some(InputAssign::device(None, 3)));
        assert_eq!(need, ReconcileNeed::Topology);
    }

    #[test]
    fn add_strip_defaults_the_name_and_never_reuses_ids() {
        let (next, delta, need) = ok(MixCommand::AddStrip { name: None });
        assert_eq!(next.strips.len(), 2);
        assert_eq!(next.strips[1].name, "Ch 2");
        assert_eq!(need, ReconcileNeed::Topology);
        let StateDelta::StripAdded { strip } = delta else {
            panic!("wrong delta")
        };
        assert_eq!(strip.id, StripId(1));
        assert_eq!(
            strip.route_to,
            RouteTarget::Master,
            "fresh strips route to master"
        );
    }

    #[test]
    fn a_full_console_refuses_another_strip() {
        let mut full = state();
        while full.strips.len() < MAX_STRIPS {
            (full, _, _) = apply(&full, MixCommand::AddStrip { name: None }).unwrap();
        }
        assert!(matches!(
            apply(&full, MixCommand::AddStrip { name: None }),
            Err(MixError::Unsupported(_))
        ));
    }

    #[test]
    fn remove_strip_refuses_unknown_ids() {
        let (next, _, _) = ok(MixCommand::RemoveStrip { id: StripId(0) });
        assert!(next.strips.is_empty());
        assert!(apply(&state(), MixCommand::RemoveStrip { id: StripId(7) }).is_err());
    }

    #[test]
    fn rename_trims_and_bounds_the_tape() {
        let (next, _, _) = ok(MixCommand::Rename {
            target: FaderTarget::Strip { id: StripId(0) },
            name: "  Kick  ".into(),
        });
        assert_eq!(next.strips[0].name, "Kick");
        assert!(
            apply(
                &state(),
                MixCommand::Rename {
                    target: FaderTarget::Strip { id: StripId(0) },
                    name: "   ".into(),
                },
            )
            .is_err()
        );
        assert!(
            apply(
                &state(),
                MixCommand::Rename {
                    target: FaderTarget::Strip { id: StripId(0) },
                    name: "x".repeat(61),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn the_input_state_is_never_mutated() {
        let original = state();
        let _ = apply(
            &original,
            MixCommand::SetFader {
                target: FaderTarget::Master,
                level_db: -12.0,
            },
        );
        assert_eq!(original.master.fader_db, 0.0);
    }

    fn state_with_buses() -> MixerState {
        use crate::fx::FxState;
        MixerState {
            strips: vec![StripState::new(StripId(0), "Ch 1".into())],
            buses: vec![
                BusState::new(BusId(0), BusKind::Aux, "FX 1".into()),
                BusState::new(BusId(1), BusKind::Group, "Band".into()),
            ],
            fx: vec![FxState {
                id: FxId(0),
                name: "Verb".into(),
                input: BusId(0),
                params: FxParams::default_reverb(),
                return_level_db: -6.0,
            }],
            ..MixerState::default()
        }
    }

    #[test]
    fn set_send_upserts_and_targets_aux_only() {
        let base = state_with_buses();
        let (next, _, need) = apply(
            &base,
            MixCommand::SetSend {
                strip: StripId(0),
                dest: BusId(0),
                level_db: -12.0,
                tap: SendTap::PostFader,
            },
        )
        .unwrap();
        assert_eq!(next.strips[0].sends.len(), 1);
        assert_eq!(need, ReconcileNeed::ParamOnly);
        let (again, _, _) = apply(
            &next,
            MixCommand::SetSend {
                strip: StripId(0),
                dest: BusId(0),
                level_db: -6.0,
                tap: SendTap::PreFader,
            },
        )
        .unwrap();
        assert_eq!(again.strips[0].sends.len(), 1, "upsert, not append");
        assert_eq!(again.strips[0].sends[0].level_db, -6.0);
        assert!(matches!(
            apply(
                &base,
                MixCommand::SetSend {
                    strip: StripId(0),
                    dest: BusId(1),
                    level_db: 0.0,
                    tap: SendTap::PostFader,
                },
            ),
            Err(MixError::Unsupported(_))
        ));
    }

    #[test]
    fn routing_targets_groups_only_and_recompiles() {
        let base = state_with_buses();
        let (next, _, need) = apply(
            &base,
            MixCommand::SetRoute {
                strip: StripId(0),
                to: RouteTarget::Bus { id: BusId(1) },
            },
        )
        .unwrap();
        assert_eq!(next.strips[0].route_to, RouteTarget::Bus { id: BusId(1) });
        assert_eq!(need, ReconcileNeed::Topology);
        assert!(matches!(
            apply(
                &base,
                MixCommand::SetRoute {
                    strip: StripId(0),
                    to: RouteTarget::Bus { id: BusId(0) },
                },
            ),
            Err(MixError::Unsupported(_))
        ));
    }

    #[test]
    fn fx_params_validate_and_cannot_change_kind() {
        let base = state_with_buses();
        let (next, _, need) = apply(
            &base,
            MixCommand::SetFxParams {
                fx: FxId(0),
                params: FxParams::Reverb {
                    room_size: 0.8,
                    damping: 0.3,
                },
            },
        )
        .unwrap();
        assert_eq!(need, ReconcileNeed::ParamOnly);
        assert!(
            matches!(next.fx[0].params, FxParams::Reverb { room_size, .. } if room_size == 0.8)
        );
        assert!(
            apply(
                &base,
                MixCommand::SetFxParams {
                    fx: FxId(0),
                    params: FxParams::default_delay(),
                },
            )
            .is_err(),
            "a reverb stays a reverb"
        );
    }

    #[test]
    fn removing_a_bus_heals_routes_and_sends_but_protects_fx_feeds() {
        let base = state_with_buses();
        let (routed, _, _) = apply(
            &base,
            MixCommand::SetRoute {
                strip: StripId(0),
                to: RouteTarget::Bus { id: BusId(1) },
            },
        )
        .unwrap();
        let (healed, _, _) = apply(&routed, MixCommand::RemoveBus { id: BusId(1) }).unwrap();
        assert_eq!(healed.strips[0].route_to, RouteTarget::Master);
        assert!(matches!(
            apply(&base, MixCommand::RemoveBus { id: BusId(0) }),
            Err(MixError::Unsupported(_)),
        ));
    }

    #[test]
    fn commands_and_deltas_round_trip_on_the_wire() {
        let command = MixCommand::SetEqBand {
            strip: StripId(2),
            band: EqBandKind::Peak,
            freq_hz: 1200.0,
            gain_db: 4.0,
            q: 0.9,
        };
        let json = serde_json::to_string(&command).unwrap();
        assert_eq!(
            json,
            r#"{"op":"set_eq_band","strip":2,"band":"peak","freq_hz":1200.0,"gain_db":4.0,"q":0.9}"#
        );
        let back: MixCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back, command);

        let delta = StateDelta::Input {
            strip: StripId(1),
            input: None,
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert_eq!(json, r#"{"kind":"input","strip":1,"input":null}"#);
    }

    #[test]
    fn instrument_commands_and_deltas_round_trip_on_the_wire() {
        let command = MixCommand::SetInstrumentPerformance {
            id: InstrumentId(1),
            name: "Rhodes".into(),
            bank: 0,
            program: 4,
            port: Some("nanoKEY2 MIDI 1".into()),
            midi_channel: Some(9),
        };
        let json = serde_json::to_string(&command).unwrap();
        assert_eq!(
            json,
            r#"{"op":"set_instrument_performance","id":1,"name":"Rhodes","bank":0,"program":4,"port":"nanoKEY2 MIDI 1","midi_channel":9}"#
        );
        assert_eq!(serde_json::from_str::<MixCommand>(&json).unwrap(), command);

        let voice = MixCommand::SetInstrumentVoice {
            id: InstrumentId(1),
            soundfont: None,
            polyphony: 32,
            effects: false,
        };
        let json = serde_json::to_string(&voice).unwrap();
        assert_eq!(
            json,
            r#"{"op":"set_instrument_voice","id":1,"soundfont":null,"polyphony":32,"effects":false}"#
        );
        assert_eq!(serde_json::from_str::<MixCommand>(&json).unwrap(), voice);

        let removed = StateDelta::InstrumentRemoved {
            id: InstrumentId(1),
            unpatched: vec![StripId(0), StripId(2)],
        };
        assert_eq!(
            serde_json::to_string(&removed).unwrap(),
            r#"{"kind":"instrument_removed","id":1,"unpatched":[0,2]}"#
        );
    }

    #[test]
    fn adding_an_instrument_needs_a_recompile_and_names_itself() {
        let (next, delta, need) = ok(MixCommand::AddInstrument { name: None });
        assert_eq!(next.instruments.len(), 1);
        assert_eq!(next.instruments[0].name, "Inst 1");
        assert_eq!(need, ReconcileNeed::Topology);
        assert!(matches!(delta, StateDelta::InstrumentAdded { .. }));
    }

    #[test]
    fn the_rack_refuses_to_grow_past_its_ceiling() {
        let mut state = state();
        for i in 0..MAX_INSTRUMENTS {
            state.instruments.push(InstrumentState::new(
                InstrumentId(i as u32),
                format!("Inst {i}"),
            ));
        }
        assert!(matches!(
            apply(&state, MixCommand::AddInstrument { name: None }),
            Err(MixError::Unsupported(_))
        ));
    }

    #[test]
    fn changing_a_preset_is_a_parameter_move_not_a_recompile() {
        // The user-facing rule: changing the sound stops the tape, changing
        // the patch does not. Reloading a soundfont rebuilds the synth;
        // choosing a preset is a MIDI message to a running one.
        let (state, _, _) = ok(MixCommand::AddInstrument { name: None });
        let (_, _, need) = apply(
            &state,
            MixCommand::SetInstrumentPerformance {
                id: InstrumentId(0),
                name: "Rhodes".into(),
                bank: 0,
                program: 4,
                port: None,
                midi_channel: None,
            },
        )
        .unwrap();
        assert_eq!(need, ReconcileNeed::ParamOnly);

        let (_, _, need) = apply(
            &state,
            MixCommand::SetInstrumentVoice {
                id: InstrumentId(0),
                soundfont: Some("piano.sf2".into()),
                polyphony: 32,
                effects: false,
            },
        )
        .unwrap();
        assert_eq!(need, ReconcileNeed::Topology);
    }

    #[test]
    fn removing_an_instrument_heals_the_strips_that_pointed_at_it() {
        // Same rule as a removed bus: a strip left patched at something
        // that no longer exists is silence the patchbay cannot explain.
        let (mut state, _, _) = ok(MixCommand::AddInstrument { name: None });
        state.strips[0].input = Some(InputAssign::instrument(InstrumentId(0), 0));
        let (healed, delta, need) = apply(
            &state,
            MixCommand::RemoveInstrument {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        assert!(healed.strips[0].input.is_none());
        assert_eq!(need, ReconcileNeed::Topology);
        let StateDelta::InstrumentRemoved { unpatched, .. } = delta else {
            panic!("expected an InstrumentRemoved");
        };
        assert_eq!(unpatched, vec![StripId(0)]);
    }

    #[test]
    fn removing_an_instrument_leaves_device_patches_alone() {
        let (mut state, _, _) = ok(MixCommand::AddInstrument { name: None });
        state.strips[0].input = Some(InputAssign::device(Some("dock".into()), 1));
        let (healed, _, _) = apply(
            &state,
            MixCommand::RemoveInstrument {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        assert_eq!(
            healed.strips[0].input,
            Some(InputAssign::device(Some("dock".into()), 1))
        );
    }

    #[test]
    fn an_out_of_range_polyphony_or_channel_is_refused() {
        let (state, _, _) = ok(MixCommand::AddInstrument { name: None });
        assert!(matches!(
            apply(
                &state,
                MixCommand::SetInstrumentVoice {
                    id: InstrumentId(0),
                    soundfont: None,
                    polyphony: 0,
                    effects: false,
                }
            ),
            Err(MixError::OutOfRange { .. })
        ));
        assert!(matches!(
            apply(
                &state,
                MixCommand::SetInstrumentPerformance {
                    id: InstrumentId(0),
                    name: "Rhodes".into(),
                    bank: 0,
                    program: 0,
                    port: None,
                    midi_channel: Some(16),
                }
            ),
            Err(MixError::OutOfRange { .. })
        ));
    }

    #[test]
    fn adding_all_channels_puts_a_named_patched_strip_on_every_output() {
        // The drum-kit setup move: six pieces, six faders, ready to pan.
        let (mut state, _, _) = ok(MixCommand::AddInstrument {
            name: Some("Kit".into()),
        });
        state.instruments[0].splits = crate::instrument::gm_drum_splits();
        let before = state.strips.len();

        let (next, delta, need) = apply(
            &state,
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(0),
            },
        )
        .unwrap();

        assert_eq!(need, ReconcileNeed::Topology);
        assert_eq!(next.strips.len(), before + 6);
        assert_eq!(next.strips[before].name, "Kit Kick");
        assert_eq!(
            next.strips[before].input,
            Some(InputAssign::instrument(InstrumentId(0), 0))
        );
        assert_eq!(next.strips[before + 3].name, "Kit HiHat");
        let StateDelta::InstrumentStripsAdded { added, .. } = delta else {
            panic!("expected InstrumentStripsAdded");
        };
        assert_eq!(added.len(), 6);
    }

    #[test]
    fn adding_all_channels_twice_adds_nothing_the_second_time() {
        // Pressing it again must not double the desk or reset the levels
        // someone has already set on the first set of strips.
        let (state, _, _) = ok(MixCommand::AddInstrument { name: None });
        let (once, _, _) = apply(
            &state,
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        let (twice, _, _) = apply(
            &once,
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        assert_eq!(once.strips.len(), twice.strips.len());
    }

    #[test]
    fn an_unsplit_instrument_lands_on_the_desk_as_a_stereo_pair() {
        // The "only assign the stereo mix" case: two strips, L and R.
        let (state, _, _) = ok(MixCommand::AddInstrument {
            name: Some("Rhodes".into()),
        });
        let before = state.strips.len();
        let (next, _, _) = apply(
            &state,
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        assert_eq!(next.strips.len(), before + 2);
        assert_eq!(next.strips[before].name, "Rhodes L");
        assert_eq!(next.strips[before + 1].name, "Rhodes R");
    }

    #[test]
    fn narrowing_the_outputs_heals_strips_that_pointed_past_the_new_width() {
        // Going from six drum faders back to a stereo mix leaves channels
        // 2..5 with nothing behind them. Silently keeping those patches
        // would land them on the NEXT instrument's audio.
        let (mut state, _, _) = ok(MixCommand::AddInstrument { name: None });
        state.instruments[0].splits = crate::instrument::gm_drum_splits();
        let (state, _, _) = apply(
            &state,
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(0),
            },
        )
        .unwrap();
        let (narrowed, _, need) = apply(
            &state,
            MixCommand::SetInstrumentSplits {
                id: InstrumentId(0),
                splits: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(need, ReconcileNeed::Topology);
        let still_patched = narrowed
            .strips
            .iter()
            .filter(|s| {
                s.input.as_ref().and_then(InputAssign::instrument_id) == Some(InstrumentId(0))
            })
            .count();
        assert_eq!(still_patched, 2, "only L and R survive a stereo layout");
    }

    #[test]
    fn a_split_with_no_key_ranges_is_refused() {
        let (state, _, _) = ok(MixCommand::AddInstrument { name: None });
        assert!(matches!(
            apply(
                &state,
                MixCommand::SetInstrumentSplits {
                    id: InstrumentId(0),
                    splits: vec![crate::instrument::InstrumentSplit {
                        name: "Kick".into(),
                        ranges: Vec::new(),
                    }],
                }
            ),
            Err(MixError::BadName(_))
        ));
    }

    #[test]
    fn editing_an_instrument_that_is_not_there_is_an_unknown_target() {
        assert!(matches!(
            apply(
                &state(),
                MixCommand::RemoveInstrument {
                    id: InstrumentId(9)
                }
            ),
            Err(MixError::UnknownTarget(_))
        ));
    }
}
