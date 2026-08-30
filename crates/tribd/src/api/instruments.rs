use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use trib_core::{InstrumentId, InstrumentSplit, InstrumentState, MixCommand, StateDelta};
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::api::strips::map_mix_err;
use crate::instrument_host::{InstrumentReport, MidiPortReport};
use crate::soundfonts::SoundfontInfo;

/// Everything the Instruments tab needs in one read: what the console
/// holds, why each one is or is not sounding, what MIDI is available, and
/// what there is to load.
///
/// One document rather than three endpoints because the three answers are
/// entangled — deleting a soundfont changes an instrument's status, and
/// unplugging a keyboard changes its reason. Split reads would let the
/// console render a pair that never existed together.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InstrumentsDto {
    /// The console document's instruments, in rack order.
    pub instruments: Vec<InstrumentState>,
    /// Machine truth about each one, keyed by the same id.
    pub reports: Vec<InstrumentReport>,
    pub midi_ports: Vec<MidiPortReport>,
    pub soundfonts: Vec<SoundfontInfo>,
    /// Where uploads land, for the console to print.
    pub soundfont_dir: String,
    /// Ceiling on an upload, so the console can refuse a file before
    /// spending twenty minutes sending it.
    pub max_upload_bytes: u64,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewInstrument {
    pub name: Option<String>,
    /// A library id to give it straight away, so a new instrument can make
    /// a sound without a second round trip. Optional: an instrument with
    /// no voice is a valid, silent one, and the console says so.
    pub soundfont: Option<String>,
}

/// Any subset of an instrument's settings. Absent fields stay put.
///
/// The daemon fans this out into the two commands the reducer draws a line
/// between — voice settings rebuild the synth and are refused while the
/// tape rolls, performance settings do not and are not.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InstrumentPatch {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub soundfont: Option<Option<String>>,
    pub bank: Option<u16>,
    pub program: Option<u8>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<String>)]
    pub port: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<u8>)]
    pub midi_channel: Option<Option<u8>>,
    pub polyphony: Option<u16>,
    pub effects: Option<bool>,
}

/// Absent field (outer None: leave alone) vs explicit `null` (Some(None):
/// clear it) — the `StripPatch.input` idiom, for the same reason: choosing
/// no soundfont and not mentioning the soundfont are different requests.
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

/// Split a patch into the commands it implies, against the instrument as
/// it currently stands.
///
/// Pure so the split — which decides what the tape refuses — is testable
/// without a running daemon.
pub fn patch_commands(current: &InstrumentState, patch: InstrumentPatch) -> Vec<MixCommand> {
    let mut commands = Vec::new();

    let soundfont = patch.soundfont.unwrap_or_else(|| current.soundfont.clone());
    let polyphony = patch.polyphony.unwrap_or(current.polyphony);
    let effects = patch.effects.unwrap_or(current.effects);
    let voice_moved = soundfont != current.soundfont
        || polyphony != current.polyphony
        || effects != current.effects;
    if voice_moved {
        commands.push(MixCommand::SetInstrumentVoice {
            id: current.id,
            soundfont,
            polyphony,
            effects,
        });
    }

    let name = patch.name.unwrap_or_else(|| current.name.clone());
    let bank = patch.bank.unwrap_or(current.bank);
    let program = patch.program.unwrap_or(current.program);
    let port = patch.port.unwrap_or_else(|| current.port.clone());
    let midi_channel = patch.midi_channel.unwrap_or(current.midi_channel);
    let performance_moved = name != current.name
        || bank != current.bank
        || program != current.program
        || port != current.port
        || midi_channel != current.midi_channel;
    if performance_moved {
        commands.push(MixCommand::SetInstrumentPerformance {
            id: current.id,
            name,
            bank,
            program,
            port,
            midi_channel,
        });
    }

    commands
}

pub(crate) async fn document(state: &AppState) -> InstrumentsDto {
    let snapshot = state.control.snapshot().await;
    let midi = state.instruments.report().await;
    InstrumentsDto {
        instruments: snapshot.instruments.clone(),
        reports: midi.instruments,
        midi_ports: midi.inputs,
        soundfonts: state.instruments.library().await,
        soundfont_dir: state.soundfont_dir.clone(),
        max_upload_bytes: state.max_soundfont_bytes,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/instruments",
    responses((status = 200, description = "The rack, its status, MIDI ports and the library", body = InstrumentsDto))
)]
pub async fn list_instruments(State(state): State<AppState>) -> Json<InstrumentsDto> {
    Json(document(&state).await)
}

/// Re-enumerate MIDI ports and retry anything that failed to load.
///
/// The manual belt, exactly like the patchbay's Refresh: nothing retries in
/// the background, so plugging a keyboard in and pressing this is the whole
/// recovery story.
#[utoipa::path(
    post,
    path = "/api/v1/instruments/refresh",
    responses((status = 200, description = "The rack after re-enumeration", body = InstrumentsDto))
)]
pub async fn refresh_instruments(State(state): State<AppState>) -> Json<InstrumentsDto> {
    state.instruments.refresh().await;
    Json(document(&state).await)
}

#[utoipa::path(
    post,
    path = "/api/v1/instruments",
    request_body = NewInstrument,
    responses(
        (status = 201, description = "The instrument, silent until it is given a soundfont", body = InstrumentState),
        (status = 409, description = "Recording, or the rack is full"),
    ),
)]
pub async fn create_instrument(
    State(state): State<AppState>,
    Json(new): Json<NewInstrument>,
) -> Result<(StatusCode, Json<InstrumentState>), ApiError> {
    let delta = state
        .control
        .apply(MixCommand::AddInstrument { name: new.name }, None)
        .await
        .map_err(map_mix_err)?;
    let StateDelta::InstrumentAdded { instrument } = delta else {
        return Err(ApiError::Internal(
            "AddInstrument produced a foreign delta".into(),
        ));
    };
    let Some(soundfont) = new.soundfont else {
        return Ok((StatusCode::CREATED, Json(instrument)));
    };
    // A second reducer command rather than a field on `AddInstrument`:
    // `SetInstrumentVoice` is where loading a soundfont already lives, and
    // teaching the core a second way to do it would be two paths to keep
    // agreeing. Both run inside this one request, so the console never
    // renders the instrument in its voiceless intermediate state.
    //
    // Polyphony and effects are read back from the instrument the reducer
    // just made, so its defaults are carried rather than restated here.
    let delta = state
        .control
        .apply(
            MixCommand::SetInstrumentVoice {
                id: instrument.id,
                soundfont: Some(soundfont),
                polyphony: instrument.polyphony,
                effects: instrument.effects,
            },
            None,
        )
        .await
        .map_err(map_mix_err)?;
    let StateDelta::InstrumentChanged { instrument } = delta else {
        return Err(ApiError::Internal(
            "SetInstrumentVoice produced a foreign delta".into(),
        ));
    };
    Ok((StatusCode::CREATED, Json(instrument)))
}

#[utoipa::path(
    put,
    path = "/api/v1/instruments/{id}",
    params(("id" = u32, Path, description = "Instrument id")),
    request_body = InstrumentPatch,
    responses(
        (status = 200, description = "The instrument after the patch", body = InstrumentState),
        (status = 404, description = "No such instrument"),
        (status = 409, description = "A voice change while recording"),
        (status = 422, description = "A value out of range"),
    ),
)]
pub async fn update_instrument(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(patch): Json<InstrumentPatch>,
) -> Result<Json<InstrumentState>, ApiError> {
    let id = InstrumentId(id);
    let snapshot = state.control.snapshot().await;
    let current = snapshot.instrument(id).ok_or(ApiError::NotFound)?;
    let commands = patch_commands(current, patch);
    if commands.is_empty() {
        return Err(ApiError::Invalid("empty patch".into()));
    }
    for command in commands {
        state
            .control
            .apply(command, None)
            .await
            .map_err(map_mix_err)?;
    }
    let snapshot = state.control.snapshot().await;
    let instrument = snapshot.instrument(id).ok_or(ApiError::NotFound)?.clone();
    Ok(Json(instrument))
}

#[utoipa::path(
    delete,
    path = "/api/v1/instruments/{id}",
    params(("id" = u32, Path, description = "Instrument id")),
    responses(
        (status = 200, description = "The rack after the removal", body = InstrumentsDto),
        (status = 404, description = "No such instrument"),
        (status = 409, description = "Recording in progress"),
    ),
)]
pub async fn delete_instrument(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<InstrumentsDto>, ApiError> {
    state
        .control
        .apply(
            MixCommand::RemoveInstrument {
                id: InstrumentId(id),
            },
            None,
        )
        .await
        .map_err(map_mix_err)?;
    Ok(Json(document(&state).await))
}

/// How an instrument reaches the desk.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputLayout {
    /// One stereo pair — a piano, a pad, anything whose stereo image is
    /// the point.
    StereoMix,
    /// The General MIDI drum map as named faders: Kick, Snare, Toms,
    /// HiHat, Cymbals, Percussion.
    GmDrums,
    /// Whatever splits the caller names.
    Custom { splits: Vec<InstrumentSplit> },
}

#[utoipa::path(
    put,
    path = "/api/v1/instruments/{id}/outputs",
    params(("id" = u32, Path, description = "Instrument id")),
    request_body = OutputLayout,
    responses(
        (status = 200, description = "The instrument after the change", body = InstrumentState),
        (status = 404, description = "No such instrument"),
        (status = 409, description = "Recording in progress"),
        (status = 422, description = "A split with no keys, or a bad range"),
    ),
)]
pub async fn set_instrument_outputs(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(layout): Json<OutputLayout>,
) -> Result<Json<InstrumentState>, ApiError> {
    let id = InstrumentId(id);
    let splits = match layout {
        OutputLayout::StereoMix => Vec::new(),
        OutputLayout::GmDrums => trib_core::gm_drum_splits(),
        OutputLayout::Custom { splits } => splits,
    };
    state
        .control
        .apply(MixCommand::SetInstrumentSplits { id, splits }, None)
        .await
        .map_err(map_mix_err)?;
    let snapshot = state.control.snapshot().await;
    Ok(Json(
        snapshot.instrument(id).ok_or(ApiError::NotFound)?.clone(),
    ))
}

/// Put every one of an instrument's channels on the desk.
///
/// The move a drum kit needs before anyone plays: one named, patched strip
/// per piece, ready to pan and fade. Only missing channels are added, so
/// pressing it twice costs nothing and existing levels survive.
#[utoipa::path(
    post,
    path = "/api/v1/instruments/{id}/strips",
    params(("id" = u32, Path, description = "Instrument id")),
    responses(
        (status = 200, description = "The strips that were added", body = [trib_core::StripState]),
        (status = 404, description = "No such instrument"),
        (status = 409, description = "Recording, or the console is full"),
    ),
)]
pub async fn add_instrument_strips(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<Vec<trib_core::StripState>>, ApiError> {
    let delta = state
        .control
        .apply(
            MixCommand::AddInstrumentStrips {
                id: InstrumentId(id),
            },
            None,
        )
        .await
        .map_err(map_mix_err)?;
    let StateDelta::InstrumentStripsAdded { added, .. } = delta else {
        return Err(ApiError::Internal(
            "AddInstrumentStrips produced a foreign delta".into(),
        ));
    };
    Ok(Json(added))
}

/// Play one note on an instrument.
///
/// The cheap half of "why is this silent?": it bisects a dead keyboard from
/// a dead instrument without any live MIDI at all.
#[utoipa::path(
    post,
    path = "/api/v1/instruments/{id}/test",
    params(("id" = u32, Path, description = "Instrument id")),
    responses(
        (status = 204, description = "A note was played"),
        (status = 404, description = "No such instrument"),
    ),
)]
pub async fn test_instrument(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<StatusCode, ApiError> {
    if state.instruments.test_note(id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Silence every held note on every instrument.
///
/// Every synthesiser ships one of these because a lost note-off is a note
/// that hangs until something makes it stop.
#[utoipa::path(
    post,
    path = "/api/v1/instruments/panic",
    responses((status = 204, description = "Every held note released"))
)]
pub async fn panic_instruments(State(state): State<AppState>) -> StatusCode {
    state.instruments.panic();
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> InstrumentState {
        let mut state = InstrumentState::new(InstrumentId(1), "Rhodes".into());
        state.soundfont = Some("piano.sf2".into());
        state.program = 4;
        state.port = Some("nanoKEY2 MIDI 1".into());
        state
    }

    fn patch(json: &str) -> Vec<MixCommand> {
        patch_commands(&current(), serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_preset_change_is_a_performance_command_only() {
        // The tape must not stop for this: it is the change a player makes
        // mid-song.
        let commands = patch(r#"{"program":9}"#);
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            commands[0],
            MixCommand::SetInstrumentPerformance { program: 9, .. }
        ));
    }

    #[test]
    fn a_soundfont_change_is_a_voice_command_only() {
        let commands = patch(r#"{"soundfont":"organ.sf2"}"#);
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], MixCommand::SetInstrumentVoice { .. }));
    }

    #[test]
    fn clearing_a_soundfont_is_different_from_not_mentioning_it() {
        assert!(
            patch(r#"{"name":"Keys"}"#)
                .iter()
                .all(|c| !matches!(c, MixCommand::SetInstrumentVoice { .. }))
        );
        let cleared = patch(r#"{"soundfont":null}"#);
        assert!(matches!(
            cleared.as_slice(),
            [MixCommand::SetInstrumentVoice {
                soundfont: None,
                ..
            }]
        ));
    }

    #[test]
    fn a_patch_touching_both_halves_emits_both_commands() {
        let commands = patch(r#"{"soundfont":"organ.sf2","program":2}"#);
        assert_eq!(commands.len(), 2);
        assert!(matches!(commands[0], MixCommand::SetInstrumentVoice { .. }));
        assert!(matches!(
            commands[1],
            MixCommand::SetInstrumentPerformance { .. }
        ));
    }

    #[test]
    fn a_patch_that_changes_nothing_emits_nothing() {
        // Otherwise a console that PUTs its whole form on every keystroke
        // would stop the tape by writing back the soundfont it already has.
        assert!(patch(r#"{"program":4,"soundfont":"piano.sf2"}"#).is_empty());
        assert!(patch(r#"{}"#).is_empty());
    }

    #[test]
    fn clearing_a_midi_channel_returns_the_instrument_to_omni() {
        let mut listening = current();
        listening.midi_channel = Some(9);
        let commands = patch_commands(
            &listening,
            serde_json::from_str(r#"{"midi_channel":null}"#).unwrap(),
        );
        assert!(matches!(
            commands.as_slice(),
            [MixCommand::SetInstrumentPerformance {
                midi_channel: None,
                ..
            }]
        ));
    }
}
