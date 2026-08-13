use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::header;
use axum::response::IntoResponse;
use serde::Serialize;
use utoipa::ToSchema;

use super::{ApiError, AppState};

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TakeTrackDto {
    pub file: String,
    pub channels: u16,
    pub frames: u64,
    pub dropped_samples: u64,
    /// The strip this track tapped — absent for the master mix and for
    /// takes cut before the field existed.
    pub strip_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TakeDto {
    pub take: u32,
    pub started_at_unix: u64,
    pub sample_rate: u32,
    pub damaged: bool,
    pub duration_secs: f64,
    pub tracks: Vec<TakeTrackDto>,
}

/// Finished takes, newest first. Read fresh from disk on every call — the
/// manifest files are the source of truth and the list is short.
#[utoipa::path(
    get,
    path = "/api/v1/takes",
    responses((status = 200, description = "Recorded takes, newest first", body = [TakeDto]))
)]
pub async fn list_takes(State(state): State<AppState>) -> Json<Vec<TakeDto>> {
    let project = state.project.borrow().clone();
    Json(take_dtos(&project))
}

/// Permanently remove one take — audio, peaks and manifest together.
///
/// Refused while recording, and refused for the take being played: the
/// feeder threads hold open readers and reopen the files on every loop
/// pass, so unlinking underneath them would run playback off unlinked
/// inodes until a later wrap died somewhere confusing.
#[utoipa::path(
    delete,
    path = "/api/v1/takes/{take}",
    params(("take" = u32, Path, description = "Take number")),
    responses(
        (status = 200, description = "Deleted; the remaining takes, newest first", body = [TakeDto]),
        (status = 404, description = "No such take"),
        (status = 409, description = "Recording, or that take is playing"),
    )
)]
pub async fn delete_take(
    State(state): State<AppState>,
    UrlPath(take): UrlPath<u32>,
) -> Result<Json<Vec<TakeDto>>, ApiError> {
    state
        .control
        .delete_take(take)
        .await
        .map_err(super::transport::map_transport_err)?;
    let project = state.project.borrow().clone();
    let takes = tokio::task::spawn_blocking(move || take_dtos(&project))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(takes))
}

/// The take list, shaped for the wire. Shared by the GET, the delete
/// handler and the control task's push, so a polled list and a pushed one
/// can never disagree. Blocking — reads every take manifest.
pub fn take_dtos(project: &trib_project::Project) -> Vec<TakeDto> {
    trib_project::list_takes(project)
        .into_iter()
        .map(|info| {
            // The longest track defines the take's length.
            let frames = info.tracks.iter().map(|t| t.frames).max().unwrap_or(0);
            TakeDto {
                take: info.take,
                started_at_unix: info.started_at_unix,
                sample_rate: info.sample_rate,
                damaged: info.damaged,
                duration_secs: frames as f64 / f64::from(info.sample_rate),
                tracks: info
                    .tracks
                    .into_iter()
                    .map(|t| TakeTrackDto {
                        file: t.file,
                        channels: t.channels,
                        frames: t.frames,
                        dropped_samples: t.dropped_samples,
                        strip_id: t.strip_id,
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Assemble the binary peaks body for one take (see the endpoint doc for
/// the layout). Pure over the pairs, so the shape is testable without IO.
fn peaks_body(samples_per_bin: u32, tracks: &[Vec<(i16, i16)>]) -> Vec<u8> {
    let pair_bytes: usize = tracks.iter().map(|t| t.len() * 4).sum();
    let mut body = Vec::with_capacity(12 + tracks.len() * 4 + pair_bytes);
    body.extend_from_slice(b"TPKS");
    body.push(1); // version
    body.push(0); // pad
    body.extend_from_slice(&(tracks.len() as u16).to_le_bytes());
    body.extend_from_slice(&samples_per_bin.to_le_bytes());
    for track in tracks {
        body.extend_from_slice(&(track.len() as u32).to_le_bytes());
        for &(min, max) in track {
            body.extend_from_slice(&min.to_le_bytes());
            body.extend_from_slice(&max.to_le_bytes());
        }
    }
    body
}

/// Waveform peaks for every track of a take, in `TakeDto.tracks` order.
///
/// Binary layout (little-endian): `"TPKS"` · `version: u8 = 1` · `pad: u8`
/// · `track_count: u16` · `samples_per_bin: u32` · then per track:
/// `bin_count: u32` + `bin_count × (min: i16, max: i16)`.
#[utoipa::path(
    get,
    path = "/api/v1/takes/{take}/peaks",
    params(("take" = u32, Path, description = "Take number")),
    responses(
        (status = 200, description = "Binary peaks document", content_type = "application/octet-stream"),
        (status = 404, description = "No such take"),
    )
)]
pub async fn take_peaks(
    State(state): State<AppState>,
    UrlPath(take): UrlPath<u32>,
) -> Result<impl IntoResponse, ApiError> {
    let project = state.project.borrow().clone();
    // Sidecar reads are cheap; legacy backfill decodes whole WAVs — either
    // way, disk work stays off the async runtime.
    let body = tokio::task::spawn_blocking(move || {
        let info = trib_project::list_takes(&project)
            .into_iter()
            .find(|t| t.take == take)
            .ok_or(ApiError::NotFound)?;
        let take_dir = project.takes_dir().join(format!("take-{take:03}"));
        // Sidecar-first per track; legacy backfills decode every file in
        // parallel inside read_or_compute.
        let files: Vec<String> = info.tracks.iter().map(|t| t.file.clone()).collect();
        let tracks = trib_project::read_or_compute(&take_dir, info.format, &files)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        Ok::<_, ApiError>(peaks_body(trib_project::PEAK_SAMPLES_PER_BIN, &tracks))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peaks_body_matches_the_documented_layout() {
        let body = peaks_body(512, &[vec![(-100, 200)], vec![(-1, 1), (-2, 2)]]);
        let mut expected = Vec::new();
        expected.extend_from_slice(b"TPKS");
        expected.extend_from_slice(&[1, 0]); // version, pad
        expected.extend_from_slice(&2u16.to_le_bytes());
        expected.extend_from_slice(&512u32.to_le_bytes());
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&(-100i16).to_le_bytes());
        expected.extend_from_slice(&200i16.to_le_bytes());
        expected.extend_from_slice(&2u32.to_le_bytes());
        for v in [-1i16, 1, -2, 2] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(body, expected);
    }
}
