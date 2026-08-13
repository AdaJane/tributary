use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::api::ws::TransportPhase;
use crate::device_host::DeviceReport;

/// The patchbay device document: every input device the OS reports, joined
/// with what the daemon has open, plus wanted-but-absent devices from the
/// loaded project. Fresh enumeration, no side effects.
#[utoipa::path(
    get,
    path = "/api/v1/devices",
    responses((status = 200, description = "Input devices joined with patch state", body = [DeviceReport]))
)]
pub async fn list_devices(State(state): State<AppState>) -> Json<Vec<DeviceReport>> {
    Json(state.devices.list().await)
}

/// The patchbay Refresh button: re-enumerate, re-run name reconciliation,
/// retry every wanted-but-unopened or failed device, and return the fresh
/// document. Also what the modal calls on open — plug and see.
#[utoipa::path(
    post,
    path = "/api/v1/devices/refresh",
    responses((status = 200, description = "Devices re-enumerated and reconciled", body = [DeviceReport]))
)]
pub async fn refresh_devices(State(state): State<AppState>) -> Json<Vec<DeviceReport>> {
    Json(state.devices.refresh().await)
}

/// Which profile to put a sound card into.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetCardProfile {
    /// The card, as `DeviceReport.card` names it. Profiles belong to the
    /// card, not to any one of its devices.
    pub card: String,
    /// One of that card's `DeviceReport.profiles[].name`.
    pub profile: String,
}

/// Switch a sound card's profile.
///
/// The active profile decides how many channels the card's devices expose,
/// so this is how an interface with more inputs than the current profile
/// shows gets the rest of them. The switch renames, resizes and remaps the
/// card's devices, so the whole document is re-enumerated and returned —
/// callers must replace their copy rather than patch it.
#[utoipa::path(
    put,
    path = "/api/v1/devices/profile",
    request_body = SetCardProfile,
    responses(
        (status = 200, description = "Devices after the profile switch", body = [DeviceReport]),
        (status = 409, description = "Recording in progress"),
        (status = 422, description = "The card or profile was refused"),
    ),
)]
pub async fn set_card_profile(
    State(state): State<AppState>,
    Json(body): Json<SetCardProfile>,
) -> Result<Json<Vec<DeviceReport>>, ApiError> {
    // Switching tears every one of the card's streams down and back up.
    // Doing that mid-take would punch a hole in the recording.
    if state.control.transport().await.state == TransportPhase::Recording {
        return Err(ApiError::Conflict(
            "stop recording before changing a device profile".into(),
        ));
    }
    state
        .devices
        .set_profile(body.card, body.profile)
        .await
        .map(Json)
        .map_err(ApiError::Invalid)
}
