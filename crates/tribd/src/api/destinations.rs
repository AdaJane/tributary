//! Candidate recording destinations: mounted drives with free space, for
//! the Setup view's destination tiles. Enumerated fresh per request; the
//! UI's Rescan button just re-GETs.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::destinations;

#[derive(Debug, Serialize, ToSchema)]
pub struct DriveDto {
    pub mount_point: String,
    pub label: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    pub read_only: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DestinationsDto {
    /// The active recording destination (projects root).
    pub current: String,
    /// The boot-config fallback root.
    pub default: String,
    /// Mounted filesystems, removable first.
    pub drives: Vec<DriveDto>,
}

#[utoipa::path(
    get,
    path = "/api/v1/destinations",
    responses((status = 200, description = "Mounted drives and the active destination", body = DestinationsDto))
)]
pub async fn list_destinations(
    State(state): State<AppState>,
) -> Result<Json<DestinationsDto>, ApiError> {
    // One snapshot for both paths — the control task already holds them
    // in absolute form.
    let snapshot = state.control.recording_settings().await;
    let drives = tokio::task::spawn_blocking(destinations::enumerate)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .into_iter()
        .map(|d| DriveDto {
            mount_point: d.mount_point,
            label: d.label,
            total_bytes: d.total_bytes,
            available_bytes: d.available_bytes,
            removable: d.removable,
            read_only: d.read_only,
        })
        .collect();
    Ok(Json(DestinationsDto {
        current: snapshot.destination,
        default: snapshot.default_destination,
        drives,
    }))
}
