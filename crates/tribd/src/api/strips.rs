use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use trib_core::{
    FaderTarget, InputAssign, InstrumentId, MasterState, MixCommand, MixError, StateDelta, StripId,
    StripState,
};
use utoipa::ToSchema;

use super::{ApiError, AppState};

/// What to patch a strip to: either a device (by OS name, `null` = the
/// system default input) or a virtual instrument, plus a channel within it.
///
/// `channel` is shared by both because it means the same thing either way,
/// and naming both `device` and `instrument` is refused rather than
/// silently resolved — a patch that quietly picks the wrong source reads as
/// a silent strip with no explanation.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InputAssignBody {
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub instrument: Option<InstrumentId>,
    pub channel: u16,
}

impl InputAssignBody {
    fn into_assign(self) -> Result<InputAssign, ApiError> {
        match (self.instrument, self.device) {
            (Some(_), Some(_)) => Err(ApiError::Invalid(
                "patch names both a device and an instrument".into(),
            )),
            (Some(instrument), None) => Ok(InputAssign::instrument(instrument, self.channel)),
            (None, device) => Ok(InputAssign::device(device, self.channel)),
        }
    }
}

/// Any subset of a strip's controls. Absent fields stay put; `input` is
/// double-optional so `null` explicitly unpatches while absence means
/// "leave it".
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StripPatch {
    pub name: Option<String>,
    pub gain_db: Option<f32>,
    pub fader_db: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    pub pfl: Option<bool>,
    pub eq_enabled: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<InputAssignBody>)]
    pub input: Option<Option<InputAssignBody>>,
}

/// Distinguish an absent field (outer None: leave alone) from an explicit
/// `null` (Some(None): unpatch) — serde folds both to None by default.
fn double_option<'de, D>(de: D) -> Result<Option<Option<InputAssignBody>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(de).map(Some)
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MasterPatch {
    pub fader_db: Option<f32>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewStrip {
    pub name: Option<String>,
}

pub fn map_mix_err(e: MixError) -> ApiError {
    match e {
        MixError::UnknownTarget(_) => ApiError::NotFound,
        MixError::OutOfRange { .. } | MixError::BadName(_) => ApiError::Invalid(e.to_string()),
        MixError::Unsupported(reason) => ApiError::Conflict(reason.into()),
    }
}

fn patch_commands(id: StripId, patch: StripPatch) -> Result<Vec<MixCommand>, ApiError> {
    let target = FaderTarget::Strip { id };
    let mut commands = Vec::new();
    if let Some(name) = patch.name {
        commands.push(MixCommand::Rename { target, name });
    }
    if let Some(gain_db) = patch.gain_db {
        commands.push(MixCommand::SetGain { strip: id, gain_db });
    }
    if let Some(level_db) = patch.fader_db {
        commands.push(MixCommand::SetFader { target, level_db });
    }
    if let Some(pan) = patch.pan {
        commands.push(MixCommand::SetPan { strip: id, pan });
    }
    if let Some(mute) = patch.mute {
        commands.push(MixCommand::SetMute { target, mute });
    }
    if let Some(on) = patch.pfl {
        commands.push(MixCommand::SetPfl { target, on });
    }
    if let Some(enabled) = patch.eq_enabled {
        commands.push(MixCommand::SetEqEnabled { strip: id, enabled });
    }
    if let Some(input) = patch.input {
        let input = input.map(InputAssignBody::into_assign).transpose()?;
        commands.push(MixCommand::SetInput { strip: id, input });
    }
    Ok(commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_patch_distinguishes_absent_null_and_device_bodies() {
        let absent: StripPatch = serde_json::from_str(r#"{"gain_db":0.0}"#).unwrap();
        assert!(absent.input.is_none(), "absent = leave the patch alone");

        let unpatch: StripPatch = serde_json::from_str(r#"{"input":null}"#).unwrap();
        assert_eq!(
            patch_commands(StripId(0), unpatch).unwrap().as_slice(),
            [MixCommand::SetInput {
                strip: StripId(0),
                input: None
            }]
        );

        let named: StripPatch =
            serde_json::from_str(r#"{"input":{"device":"dock","channel":1}}"#).unwrap();
        let MixCommand::SetInput {
            input: Some(assign),
            ..
        } = patch_commands(StripId(0), named).unwrap().remove(0)
        else {
            panic!("expected a SetInput");
        };
        assert_eq!(assign.device_name(), Some(Some("dock")));
        assert_eq!(assign.channel(), 1);

        let default_device: StripPatch =
            serde_json::from_str(r#"{"input":{"channel":0}}"#).unwrap();
        let MixCommand::SetInput {
            input: Some(assign),
            ..
        } = patch_commands(StripId(0), default_device)
            .unwrap()
            .remove(0)
        else {
            panic!("expected a SetInput");
        };
        assert_eq!(
            assign.device_name(),
            Some(None),
            "no device = the system default"
        );
    }

    #[test]
    fn an_instrument_patch_body_becomes_an_instrument_assign() {
        let body: StripPatch =
            serde_json::from_str(r#"{"input":{"instrument":2,"channel":1}}"#).unwrap();
        let MixCommand::SetInput {
            input: Some(assign),
            ..
        } = patch_commands(StripId(0), body).unwrap().remove(0)
        else {
            panic!("expected a SetInput");
        };
        assert_eq!(assign.instrument_id(), Some(InstrumentId(2)));
        assert_eq!(assign.channel(), 1);
    }

    #[test]
    fn a_patch_body_naming_both_a_device_and_an_instrument_is_refused() {
        let body: StripPatch =
            serde_json::from_str(r#"{"input":{"device":"dock","instrument":2,"channel":0}}"#)
                .unwrap();
        let err = patch_commands(StripId(0), body).unwrap_err();
        assert!(matches!(err, ApiError::Invalid(_)), "{err:?}");
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/strips",
    request_body = NewStrip,
    responses(
        (status = 201, description = "The strip, freshly taped", body = StripState),
        (status = 409, description = "The console is full"),
    ),
)]
pub async fn create_strip(
    State(state): State<AppState>,
    Json(new): Json<NewStrip>,
) -> Result<(StatusCode, Json<StripState>), ApiError> {
    let delta = state
        .control
        .apply(MixCommand::AddStrip { name: new.name }, None)
        .await
        .map_err(map_mix_err)?;
    let StateDelta::StripAdded { strip } = delta else {
        return Err(ApiError::Internal(
            "AddStrip produced a foreign delta".into(),
        ));
    };
    Ok((StatusCode::CREATED, Json(strip)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/strips/{id}",
    params(("id" = u32, Path, description = "Strip id")),
    responses(
        (status = 204, description = "Strip removed"),
        (status = 404, description = "No such strip"),
    ),
)]
pub async fn delete_strip(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<StatusCode, ApiError> {
    state
        .control
        .apply(MixCommand::RemoveStrip { id: StripId(id) }, None)
        .await
        .map_err(map_mix_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/strips/{id}",
    params(("id" = u32, Path, description = "Strip id")),
    request_body = StripPatch,
    responses(
        (status = 200, description = "The strip after the patch", body = StripState),
        (status = 404, description = "No such strip"),
        (status = 422, description = "A value out of range"),
    ),
)]
pub async fn update_strip(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(patch): Json<StripPatch>,
) -> Result<Json<StripState>, ApiError> {
    let id = StripId(id);
    let commands = patch_commands(id, patch)?;
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
    let strip = snapshot.strip(id).ok_or(ApiError::NotFound)?.clone();
    Ok(Json(strip))
}

#[utoipa::path(
    put,
    path = "/api/v1/master",
    request_body = MasterPatch,
    responses(
        (status = 200, description = "The master section after the patch", body = MasterState),
        (status = 422, description = "A value out of range"),
    ),
)]
pub async fn update_master(
    State(state): State<AppState>,
    Json(patch): Json<MasterPatch>,
) -> Result<Json<MasterState>, ApiError> {
    let Some(level_db) = patch.fader_db else {
        return Err(ApiError::Invalid("empty patch".into()));
    };
    state
        .control
        .apply(
            MixCommand::SetFader {
                target: FaderTarget::Master,
                level_db,
            },
            None,
        )
        .await
        .map_err(map_mix_err)?;
    Ok(Json(state.control.snapshot().await.master))
}
