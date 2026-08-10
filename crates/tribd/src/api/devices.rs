use axum::Json;
use axum::extract::State;

use super::AppState;
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
