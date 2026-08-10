use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use utoipa::ToSchema;

use super::ws::{LoopRegionDto, MonitorTarget, TransportDto};
use super::{ApiError, AppState};
use crate::engine_host::TransportError;

fn map_transport_err(e: TransportError) -> ApiError {
    match e {
        TransportError::AlreadyRecording => ApiError::Conflict("already recording".into()),
        TransportError::NothingArmed => {
            ApiError::Invalid("nothing armed — arm a channel or the master first".into())
        }
        TransportError::Busy(detail) => ApiError::Conflict(detail.into()),
        TransportError::NoTake => ApiError::Invalid("nothing to play — record a take first".into()),
        TransportError::NoSuchLane => ApiError::Invalid("no such lane".into()),
        TransportError::BadLoop(detail) => ApiError::Invalid(detail.into()),
        TransportError::SampleRateMismatch => ApiError::Invalid(
            "the take was recorded at a different sample rate than the engine runs".into(),
        ),
        TransportError::BadSetting(detail) => ApiError::Invalid(detail),
        TransportError::Io(detail) => ApiError::Internal(detail),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/transport",
    responses((status = 200, description = "Current transport state", body = TransportDto))
)]
pub async fn get_transport(State(state): State<AppState>) -> Json<TransportDto> {
    Json(state.control.transport().await)
}

#[utoipa::path(
    post,
    path = "/api/v1/transport/record/start",
    responses(
        (status = 200, description = "Recording", body = TransportDto),
        (status = 409, description = "Already recording"),
        (status = 422, description = "Nothing armed"),
    ),
)]
pub async fn record_start(State(state): State<AppState>) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .record_start()
        .await
        .map(Json)
        .map_err(map_transport_err)
}

#[utoipa::path(
    post,
    path = "/api/v1/transport/record/stop",
    responses((status = 200, description = "Stopped (idempotent)", body = TransportDto)),
)]
pub async fn record_stop(State(state): State<AppState>) -> Json<TransportDto> {
    Json(state.control.record_stop().await)
}

#[utoipa::path(
    post,
    path = "/api/v1/transport/play",
    responses(
        (status = 200, description = "Playing the latest take", body = TransportDto),
        (status = 409, description = "Recording"),
        (status = 422, description = "No take, or sample-rate mismatch"),
    ),
)]
pub async fn play(State(state): State<AppState>) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .play()
        .await
        .map(Json)
        .map_err(map_transport_err)
}

#[utoipa::path(
    post,
    path = "/api/v1/transport/stop",
    responses((status = 200, description = "Playback stopped (idempotent)", body = TransportDto)),
)]
pub async fn play_stop(State(state): State<AppState>) -> Json<TransportDto> {
    Json(state.control.play_stop().await)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SeekBody {
    pub position_frames: u64,
}

#[utoipa::path(
    put,
    path = "/api/v1/transport/loop",
    request_body = LoopRegionDto,
    responses(
        (status = 200, description = "Loop set (rebuilds a playing session)", body = TransportDto),
        (status = 409, description = "Recording"),
        (status = 422, description = "Region the take can't honor"),
    ),
)]
pub async fn set_loop(
    State(state): State<AppState>,
    Json(region): Json<LoopRegionDto>,
) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .set_loop(Some(region))
        .await
        .map(Json)
        .map_err(map_transport_err)
}

#[utoipa::path(
    delete,
    path = "/api/v1/transport/loop",
    responses((status = 200, description = "Loop cleared", body = TransportDto)),
)]
pub async fn clear_loop(State(state): State<AppState>) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .set_loop(None)
        .await
        .map(Json)
        .map_err(map_transport_err)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MonitorBody {
    pub target: MonitorTarget,
}

#[utoipa::path(
    put,
    path = "/api/v1/transport/monitor",
    request_body = MonitorBody,
    responses((status = 200, description = "Monitor target set", body = TransportDto)),
)]
pub async fn set_monitor(
    State(state): State<AppState>,
    Json(body): Json<MonitorBody>,
) -> Json<TransportDto> {
    Json(state.control.set_monitor(body.target).await)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LaneGateBody {
    /// Omitted = leave the flag alone.
    pub solo: Option<bool>,
    pub mute: Option<bool>,
}

#[utoipa::path(
    put,
    path = "/api/v1/transport/lanes/{index}",
    params(("index" = u32, Path, description = "Lane index within the latest take's tracks")),
    request_body = LaneGateBody,
    responses(
        (status = 200, description = "Lane gates updated", body = TransportDto),
        (status = 422, description = "No such lane"),
    ),
)]
pub async fn set_lane(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<u32>,
    Json(body): Json<LaneGateBody>,
) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .set_lane_gate(index, body.solo, body.mute)
        .await
        .map(Json)
        .map_err(map_transport_err)
}

#[utoipa::path(
    post,
    path = "/api/v1/transport/seek",
    request_body = SeekBody,
    responses(
        (status = 200, description = "Playhead moved (rebuilds the session while playing)", body = TransportDto),
        (status = 409, description = "Recording"),
    ),
)]
pub async fn seek(
    State(state): State<AppState>,
    Json(body): Json<SeekBody>,
) -> Result<Json<TransportDto>, ApiError> {
    state
        .control
        .seek(body.position_frames)
        .await
        .map(Json)
        .map_err(map_transport_err)
}
