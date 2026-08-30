pub mod audio;
pub mod destinations;
pub mod devices;
pub mod instruments;
pub mod midi;
pub mod outputs;
pub mod recording;
pub mod sessions;
pub mod soundfonts;
pub mod state;
pub mod strips;
pub mod takes;
pub mod transport;
#[cfg(feature = "embed-ui")]
pub mod ui;
pub mod ws;

use std::sync::Arc;

use axum::http::{HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use tower_http::cors::CorsLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::hub::Hub;
use crate::registry::ChannelRegistry;

#[derive(Clone)]
pub struct AppState {
    pub hub: Hub,
    pub registry: ChannelRegistry,
    /// Allowed browser origins — enforced on the WS handshake (which CORS
    /// can't fence). Shared with the REST CorsLayer.
    pub cors_origins: Arc<Vec<String>>,
    /// Route to the control task: the only way handlers mutate or read the
    /// mixer document.
    pub control: crate::engine_host::ControlHandle,
    /// The open project — read-only here (take listings); the control task
    /// owns all writes and republishes on a destination swap. Clone the
    /// Arc out of the borrow immediately; never hold it across an await.
    pub project: tokio::sync::watch::Receiver<Arc<trib_project::Project>>,
    /// Whether this installation ships the privileged storage helper.
    /// Probed once at startup: presence, the exec bit and the sudoers
    /// grant can each be missing independently, so only invoking it proves
    /// all three — and the answer cannot change while the daemon runs.
    pub can_format: bool,
    /// Whether the audio backend can drive patchable outputs at all.
    /// Probed once at boot for the same reason `can_format` is: it cannot
    /// change while the daemon runs, and answering 501 is more honest than
    /// accepting a patch nothing will ever play.
    pub supports_outputs: bool,
    /// Monitor-stream fan-out: `/ws/monitor` sockets subscribe here.
    pub monitor_tx: tokio::sync::broadcast::Sender<axum::body::Bytes>,
    /// The device orchestrator — the only path to the audio backend's
    /// DEVICES. The backend itself is below, for its own status.
    pub devices: crate::device_host::DeviceHandle,
    /// The audio backend, for `/audio`: its own state (which card, why
    /// not) is read straight off it — that read never blocks and never
    /// enumerates, so it needs no orchestrator round trip.
    pub audio: Arc<dyn trib_audio::AudioBackend>,
    pub audio_layer: crate::settings::AudioLayer,
    /// Whether `start()` returned a stream at boot. It cannot change while
    /// the daemon runs, which is why it lives here and not on the backend.
    pub audio_started: bool,
    /// The instrument orchestrator — MIDI ports and the SoundFont library.
    pub instruments: crate::instrument_host::InstrumentHandle,
    /// Where uploads land. Resolved once at boot, so handlers never
    /// re-derive it and cannot disagree about it.
    pub soundfont_dir: String,
    /// The read-only shipped library. Resolved once at boot beside
    /// `soundfont_dir`, so handlers never re-derive either and cannot
    /// disagree about where sounds live.
    pub builtin_soundfont_dir: String,
    pub max_soundfont_bytes: u64,
}

impl AppState {
    /// The soundfont search path, from the two roots resolved at boot.
    ///
    /// Rebuilt per call rather than stored: it is two `PathBuf`s, and a
    /// third copy of the same strings in `AppState` is one more thing that
    /// could drift from them.
    pub fn library(&self) -> crate::soundfonts::Library {
        crate::soundfonts::Library {
            builtin: std::path::PathBuf::from(&self.builtin_soundfont_dir),
            user: std::path::PathBuf::from(&self.soundfont_dir),
        }
    }
}

/// Errors surfaced by REST handlers.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal error: {0}")]
    Internal(String),
    /// The build or the platform cannot do this at all — not a bad request
    /// and not a transient failure. Only the appliance image ships the
    /// privileged storage helper, so a package install must say so plainly
    /// rather than failing as though the user got something wrong.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The body outgrew what this installation accepts. Distinct from
    /// `Invalid` because the file is fine — there is just too much of it,
    /// and the console's advice ("use a smaller soundfont") differs.
    #[error("too large: {0}")]
    TooLarge(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use axum::http::StatusCode;
        match &self {
            ApiError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "not_found" })),
            )
                .into_response(),
            ApiError::Invalid(detail) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "invalid", "detail": detail })),
            )
                .into_response(),
            ApiError::Conflict(detail) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "conflict", "detail": detail })),
            )
                .into_response(),
            ApiError::Unsupported(detail) => (
                StatusCode::NOT_IMPLEMENTED,
                Json(serde_json::json!({ "error": "unsupported", "detail": detail })),
            )
                .into_response(),
            ApiError::TooLarge(detail) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(serde_json::json!({ "error": "too_large", "detail": detail })),
            )
                .into_response(),
            // Internal details go to the log, never to the client.
            ApiError::Internal(detail) => {
                tracing::error!(%detail, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "internal" })),
                )
                    .into_response()
            }
        }
    }
}

#[utoipa::path(get, path = "/healthz", responses((status = 200, description = "Daemon is up")))]
async fn healthz() -> &'static str {
    "ok"
}

#[derive(OpenApi)]
#[openapi(
    info(title = "tribd", description = "Tributary daemon API"),
    components(schemas(ws::Channel, ws::ClientMessage, ws::ServerMessage, ws::WsErrorCode))
)]
struct ApiDoc;

fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(healthz))
        .routes(routes!(state::get_state))
        .routes(routes!(strips::create_strip))
        .routes(routes!(strips::update_strip, strips::delete_strip))
        .routes(routes!(strips::update_master))
        .routes(routes!(transport::get_transport))
        .routes(routes!(transport::record_start))
        .routes(routes!(transport::record_stop))
        .routes(routes!(transport::play))
        .routes(routes!(transport::play_stop))
        .routes(routes!(transport::seek))
        .routes(routes!(transport::set_loop, transport::clear_loop))
        .routes(routes!(transport::set_lane))
        .routes(routes!(transport::set_monitor))
        .routes(routes!(takes::list_takes))
        .routes(routes!(takes::take_peaks))
        .routes(routes!(audio::get_audio))
        .routes(routes!(devices::list_devices))
        .routes(routes!(devices::refresh_devices))
        .routes(routes!(devices::set_card_profile))
        .routes(routes!(outputs::list_outputs))
        .routes(routes!(outputs::refresh_outputs))
        .routes(routes!(outputs::patch_output))
        .routes(routes!(midi::list_midi))
        .routes(routes!(midi::refresh_midi))
        .routes(routes!(midi::route_midi))
        .routes(routes!(
            instruments::list_instruments,
            instruments::create_instrument
        ))
        .routes(routes!(instruments::refresh_instruments))
        .routes(routes!(instruments::panic_instruments))
        .routes(routes!(
            instruments::update_instrument,
            instruments::delete_instrument
        ))
        .routes(routes!(instruments::test_instrument))
        .routes(routes!(instruments::set_instrument_outputs))
        .routes(routes!(instruments::add_instrument_strips))
        .routes(routes!(
            soundfonts::upload_soundfont,
            soundfonts::delete_soundfont
        ))
        .routes(routes!(soundfonts::list_presets))
        .routes(routes!(
            recording::get_recording,
            recording::update_recording
        ))
        .routes(routes!(destinations::list_destinations))
        .routes(routes!(destinations::format_drive))
        .routes(routes!(sessions::list_sessions, sessions::create_session))
        .routes(routes!(sessions::rename_session, sessions::delete_session))
        .routes(routes!(sessions::open_session))
        .routes(routes!(transport::select_take))
        .routes(routes!(takes::delete_take))
}

/// The committed `openapi.json` — regenerated by `tribd openapi`, checked for
/// drift in CI, consumed by the web codegen.
pub fn openapi_json() -> String {
    let (_, api) = api_router().split_for_parts();
    api.to_pretty_json().expect("OpenAPI spec serializes")
}

pub fn build(state: AppState) -> Router {
    // Same doctrine as the WS handshake: loopback on any port, same-origin
    // from `.local`/IP-literal hosts, plus the configured extras (reverse
    // proxies, custom DNS). One function owns the rule — see ws.rs.
    let allowed = state.cors_origins.clone();
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::predicate(
            move |origin: &HeaderValue, parts| {
                let host = parts
                    .headers
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok());
                origin
                    .to_str()
                    .is_ok_and(|origin| ws::origin_allowed(Some(origin), host, &allowed))
            },
        ))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
        ]);

    let (router, _api) = api_router().split_for_parts();
    let router = router
        .route("/ws", any(ws::ws_handler))
        .route("/ws/monitor", any(ws::monitor_ws_handler));
    // The embedded console: registered routes win; everything else GETs
    // the SPA (same-origin, so the loopback origin doctrine holds).
    #[cfg(feature = "embed-ui")]
    let router = router.fallback_service(axum::routing::get(ui::serve));
    router.layer(cors).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_spec_names_the_daemon() {
        let spec = openapi_json();
        assert!(spec.contains("\"tribd\""));
        assert!(spec.contains("/healthz"));
    }
}
