use axum::Json;
use axum::extract::State;
use trib_core::MixerState;

use super::AppState;

/// The one REST snapshot: everything a client needs to render the console.
#[utoipa::path(
    get,
    path = "/api/v1/state",
    responses((status = 200, description = "Full mixer document", body = MixerState))
)]
pub async fn get_state(State(state): State<AppState>) -> Json<MixerState> {
    Json(state.control.snapshot().await)
}
