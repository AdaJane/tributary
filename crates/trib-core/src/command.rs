use serde::{Deserialize, Serialize};

use crate::bus::{BusKind, BusState};
use crate::db::{FADER_MAX_DB, FADER_MIN_DB, GAIN_MAX_DB, GAIN_MIN_DB};
use crate::eq::{EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_RANGE_DB, EQ_Q_MAX, EQ_Q_MIN, EqBandKind};
use crate::fx::FxParams;
use crate::id::{BusId, FxId, StripId};
use crate::mix::MixerState;
use crate::strip::{InputAssign, RouteTarget, SendState, SendTap, StripState};

/// Longest accepted strip/bus name — it has to fit on the tape.
pub const MAX_NAME_LEN: usize = 60;

/// Console capacity. Keeps every strip metered within the engine's fixed
/// MeterBlock (64 slots shared with buses, FX and master).
pub const MAX_STRIPS: usize = 32;

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
    },
    RecordArm {
        target: FaderTarget,
        armed: bool,
    },
    RecordArmAll {
        armed: bool,
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
            (StateDelta::StripRemoved { id }, Topology)
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
            (StateDelta::BusRemoved { id }, Topology)
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
    };
    Ok((next, delta, need))
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
            input: Some(InputAssign {
                device: None,
                device_channel: 3,
            }),
        });
        assert_eq!(
            next.strips[0].input,
            Some(InputAssign {
                device: None,
                device_channel: 3
            })
        );
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
}
