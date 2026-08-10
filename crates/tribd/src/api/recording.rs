//! Recording settings: destination, file format, and sample rate. GET is a
//! snapshot from the control task; PUT applies live where the engine
//! allows (destination, format) and restart-gates what it doesn't
//! (sample rate).

use std::path::PathBuf;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use trib_project::RecordFormat;
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::engine_host::TransportError;
use crate::settings::SAMPLE_RATES;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RecordingSettingsDto {
    /// The projects root the next take lands on.
    pub destination: String,
    /// The persisted preference — differs from `destination` when the
    /// configured drive was missing at boot and the daemon fell back.
    pub configured_destination: Option<String>,
    /// The boot-config fallback root.
    pub default_destination: String,
    /// The open project at the destination.
    pub project_name: String,
    pub format: RecordFormat,
    /// What the engine graph runs at — immutable per boot.
    pub active_sample_rate: u32,
    /// What the prefs ask for — takes effect at the next daemon start.
    pub configured_sample_rate: u32,
    pub restart_required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRecordingSettings {
    /// Absolute path of the new destination. Omitted = leave alone.
    pub destination: Option<String>,
    pub format: Option<RecordFormat>,
    pub sample_rate: Option<u32>,
}

fn map_recording_err(e: TransportError) -> ApiError {
    match e {
        TransportError::Busy(detail) => ApiError::Conflict(detail.into()),
        TransportError::BadSetting(detail) => ApiError::Invalid(detail),
        TransportError::Io(detail) => ApiError::Internal(detail),
        _ => ApiError::Internal(format!("{e:?}")),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/recording",
    responses((status = 200, description = "Current recording settings", body = RecordingSettingsDto))
)]
pub async fn get_recording(State(state): State<AppState>) -> Json<RecordingSettingsDto> {
    Json(state.control.recording_settings().await)
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/recording",
    request_body = UpdateRecordingSettings,
    responses(
        (status = 200, description = "Settings after the update", body = RecordingSettingsDto),
        (status = 409, description = "Recording in progress"),
        (status = 422, description = "Invalid destination, format, or sample rate"),
    ),
)]
pub async fn update_recording(
    State(state): State<AppState>,
    Json(body): Json<UpdateRecordingSettings>,
) -> Result<Json<RecordingSettingsDto>, ApiError> {
    if let Some(rate) = body.sample_rate
        && !SAMPLE_RATES.contains(&rate)
    {
        return Err(ApiError::Invalid(format!(
            "sample_rate must be one of {SAMPLE_RATES:?}"
        )));
    }
    let destination = match body.destination {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err(ApiError::Invalid(
                    "destination must be an absolute path".into(),
                ));
            }
            Some(path)
        }
        None => None,
    };
    state
        .control
        .update_recording(destination, body.format, body.sample_rate)
        .await
        .map(Json)
        .map_err(map_recording_err)
}
