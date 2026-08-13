//! Candidate recording destinations for the Setup view, and the format
//! endpoint that prepares one.
//!
//! Enumerated fresh per request, and pushed on the `destinations` channel
//! whenever the mount table changes — the Rescan button is now the manual
//! belt rather than the only path.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::destinations::{self, DriveState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct DriveDto {
    /// The block device, when one backs this filesystem.
    pub device: Option<String>,
    /// `null` when the drive is present but nothing mounted it — the case
    /// that used to render as no drive at all.
    pub mount_point: Option<String>,
    pub label: String,
    /// "exfat", "ext4", … `null` when the kernel recognised none.
    pub filesystem: Option<String>,
    pub total_bytes: u64,
    /// Only knowable while mounted.
    pub available_bytes: Option<u64>,
    pub removable: bool,
    pub state: DriveState,
    /// Why this is not a usable destination; `null` when it is.
    pub reason: Option<String>,
    /// The whole disk this lives on, so the console can group partitions
    /// under the drive they came from.
    pub disk: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DestinationsDto {
    /// The active recording destination (projects root).
    pub current: String,
    /// The boot-config fallback root.
    pub default: String,
    /// Every candidate filesystem, usable first — never filtered down to
    /// the usable ones. A drive that is plugged in but unusable rendering
    /// as nothing is indistinguishable from an empty port, which is the
    /// failure this endpoint exists to make impossible.
    pub drives: Vec<DriveDto>,
    /// Whether this installation can format a drive. False on package
    /// installs, which ship no privileged helper — the console shows the
    /// control disabled with a reason rather than failing on tap.
    pub can_format: bool,
}

#[derive(Debug, serde::Deserialize, ToSchema)]
pub struct FormatRequest {
    /// The whole disk, as `DriveDto.disk` names it ("/dev/sda"). A
    /// partition is refused: formatting one strands the rest of the drive.
    pub device: String,
    /// The exFAT volume label, which becomes the drive's printed name.
    pub label: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/destinations/format",
    request_body = FormatRequest,
    responses(
        (status = 200, description = "Drive formatted; it remounts on its own"),
        (status = 409, description = "Recording, or the drive is in use"),
        (status = 422, description = "Refused — not a removable whole disk, or a bad label"),
        (status = 501, description = "This installation has no format helper"),
    )
)]
pub async fn format_drive(
    State(state): State<AppState>,
    Json(req): Json<FormatRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !state.can_format {
        return Err(ApiError::Unsupported(
            "formatting is only available on the Tributary appliance".into(),
        ));
    }
    // Only the daemon knows this, which is why the helper cannot check it.
    if state.control.transport().await.state == crate::api::ws::TransportPhase::Recording {
        return Err(ApiError::Conflict(
            "stop recording before formatting a drive".into(),
        ));
    }
    let device = req.device.clone();
    let label = req.label.clone();
    tokio::task::spawn_blocking(move || crate::format::run(&device, &label))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map_err(|e| match e {
            crate::format::FormatError::Refused(d) => ApiError::Invalid(d),
            crate::format::FormatError::Busy(d) => ApiError::Conflict(d),
            crate::format::FormatError::Internal(d) => ApiError::Internal(d),
        })?;
    // The drive remounts via the automount rule, and the mount watcher
    // pushes the new list — so there is nothing to return but success.
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Enumerate and shape for the wire. Blocking — shared by the GET and the
/// mount watcher so a pushed update and a polled one can never disagree.
pub fn drive_dtos() -> Vec<DriveDto> {
    destinations::enumerate()
        .into_iter()
        .map(|d| DriveDto {
            reason: d.state.reason().map(str::to_owned),
            state: d.state,
            device: d.device,
            mount_point: d.mount_point,
            label: d.label,
            filesystem: d.filesystem,
            total_bytes: d.total_bytes,
            available_bytes: d.available_bytes,
            removable: d.removable,
            disk: d.disk,
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/v1/destinations",
    responses((status = 200, description = "Every candidate drive and the active destination", body = DestinationsDto))
)]
pub async fn list_destinations(
    State(state): State<AppState>,
) -> Result<Json<DestinationsDto>, ApiError> {
    // One snapshot for both paths — the control task already holds them
    // in absolute form.
    let snapshot = state.control.recording_settings().await;
    let drives = tokio::task::spawn_blocking(drive_dtos)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(DestinationsDto {
        can_format: state.can_format,
        current: snapshot.destination,
        default: snapshot.default_destination,
        drives,
    }))
}
