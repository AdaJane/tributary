//! Sessions: the reels of tape on the current drive.
//!
//! "Session" is the word on the wire and in the console; `Project` is the
//! word in the code. They are the same thing — the crate's own doc already
//! calls a project directory "one directory per session" — and renaming
//! the type would churn the manifest format, the config key and every take
//! path for no functional gain.
//!
//! Sessions live under the active destination, so this list is
//! per-destination by construction: changing the drive changes the list,
//! and there is no cross-drive scan.

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::transport::map_transport_err;
use super::{ApiError, AppState};
use crate::console::SessionSeed;

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct SessionDto {
    /// The directory name. Stable across a rename — renaming retitles the
    /// manifest and never moves the directory, so an id a client is
    /// holding stays valid.
    pub id: String,
    pub name: String,
    pub created_at_unix: u64,
    pub take_count: usize,
    /// Exactly one session in a list is open.
    pub open: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionsDto {
    /// The projects root these live under — the active destination.
    pub root: String,
    /// False when the destination has gone away (drive unplugged). An
    /// empty list then means "we cannot see them", not "there are none" —
    /// two different sentences the console must not merge.
    pub root_present: bool,
    /// Newest first, in the same order an adopt would pick from.
    pub sessions: Vec<SessionDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewSession {
    pub name: String,
    /// What the new session's desk starts from.
    pub seed: SessionSeed,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameSession {
    pub name: String,
}

/// Shape summaries for the wire, marking the open one.
fn to_dtos(summaries: Vec<trib_project::SessionSummary>, open_id: &str) -> Vec<SessionDto> {
    summaries
        .into_iter()
        .map(|s| SessionDto {
            open: s.id == open_id,
            id: s.id,
            name: s.name,
            created_at_unix: s.created_at_unix,
            take_count: s.take_count,
        })
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    responses((status = 200, description = "Sessions on the active destination", body = SessionsDto))
)]
pub async fn list_sessions(State(state): State<AppState>) -> Result<Json<SessionsDto>, ApiError> {
    let root = state.control.recording_settings().await.destination;
    let open = state.control.session().await;
    // Never walk a directory tree on the control task: a second of stalled
    // control loop is a second of dead faders.
    let scan_root = root.clone();
    let (present, summaries) = tokio::task::spawn_blocking(move || {
        let path = std::path::Path::new(&scan_root);
        (path.is_dir(), trib_project::list_sessions(path))
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(SessionsDto {
        root,
        root_present: present,
        sessions: to_dtos(summaries, &open.id),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions",
    request_body = NewSession,
    responses(
        (status = 200, description = "Created and opened", body = SessionDto),
        (status = 409, description = "Recording, or that name is taken"),
        (status = 422, description = "Bad name"),
    )
)]
pub async fn create_session(
    State(state): State<AppState>,
    Json(body): Json<NewSession>,
) -> Result<Json<SessionDto>, ApiError> {
    // Creating also opens: tearing off fresh tape puts it on the machine.
    let made = state
        .control
        .create_session(body.name, body.seed)
        .await
        .map_err(map_transport_err)?;
    Ok(Json(SessionDto {
        id: made.id,
        name: made.name,
        created_at_unix: 0,
        take_count: 0,
        open: true,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/open",
    params(("id" = String, Path, description = "Session directory name")),
    responses(
        (status = 200, description = "Opened", body = SessionDto),
        (status = 404, description = "No session by that id"),
        (status = 409, description = "Recording"),
    )
)]
pub async fn open_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<SessionDto>, ApiError> {
    let open = state
        .control
        .open_session(id)
        .await
        .map_err(map_transport_err)?;
    Ok(Json(SessionDto {
        id: open.id,
        name: open.name,
        created_at_unix: 0,
        take_count: 0,
        open: true,
    }))
}

#[utoipa::path(
    put,
    path = "/api/v1/sessions/{id}",
    params(("id" = String, Path, description = "Session directory name")),
    request_body = RenameSession,
    responses(
        (status = 200, description = "Renamed", body = SessionDto),
        (status = 404, description = "No session by that id"),
        (status = 422, description = "Bad name"),
    )
)]
pub async fn rename_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<RenameSession>,
) -> Result<Json<SessionDto>, ApiError> {
    let open = state.control.session().await;
    let renamed = state
        .control
        .rename_session(id, body.name)
        .await
        .map_err(map_transport_err)?;
    Ok(Json(SessionDto {
        open: open.id == renamed.id,
        id: renamed.id,
        name: renamed.name,
        created_at_unix: 0,
        take_count: 0,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{id}",
    params(("id" = String, Path, description = "Session directory name")),
    responses(
        (status = 204, description = "Deleted, permanently"),
        (status = 404, description = "No session by that id"),
        (status = 409, description = "Recording, or that session is open"),
    )
)]
pub async fn delete_session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .control
        .delete_session(id)
        .await
        .map_err(map_transport_err)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, name: &str, takes: usize) -> trib_project::SessionSummary {
        trib_project::SessionSummary {
            id: id.into(),
            name: name.into(),
            created_at_unix: 100,
            take_count: takes,
        }
    }

    #[test]
    fn exactly_one_row_is_marked_open() {
        let rows = to_dtos(
            vec![
                summary("300-new", "New", 0),
                summary("200-mid", "Mid", 4),
                summary("100-old", "Old", 2),
            ],
            "200-mid",
        );
        assert_eq!(rows.iter().filter(|r| r.open).count(), 1);
        assert!(rows[1].open);
        assert_eq!(rows[1].take_count, 4);
        // Order is preserved: newest first, as the crate returned them.
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["300-new", "200-mid", "100-old"]
        );
    }

    /// An id that matches nothing open — e.g. right after the open session
    /// was deleted from another tab — must not light a random row.
    #[test]
    fn no_row_is_open_when_the_id_matches_nothing() {
        let rows = to_dtos(vec![summary("100-old", "Old", 0)], "999-gone");
        assert!(rows.iter().all(|r| !r.open));
    }
}
