use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use trib_core::{MidiRoute, MidiSource, MixCommand};
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::api::strips::map_mix_err;
use crate::instrument_host::MidiPortReport;
use crate::midi_out::MidiOutPortReport;

/// Why one route is or is not carrying anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MidiRouteStatus {
    /// Wired to a port that is open.
    Live,
    /// Wired, but something upstream means nothing will be sent.
    Ready,
    /// The port it names is not here.
    Missing,
}

/// One route, joined with what the daemon knows about it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MidiRouteReport {
    pub port: String,
    pub status: MidiRouteStatus,
    /// The daemon's own sentence. `None` when there is nothing to explain.
    pub reason: Option<String>,
}

/// The MIDI patch bay in one read: routes, their state, and both
/// directions' ports.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MidiDto {
    pub routes: Vec<MidiRoute>,
    /// Index-aligned with `routes`.
    pub reports: Vec<MidiRouteReport>,
    pub inputs: Vec<MidiPortReport>,
    pub outputs: Vec<MidiOutPortReport>,
    /// Sidecar names the selected take carries, for the take-playback
    /// picker. A route names one of these rather than a take number,
    /// because a take number does not survive the next take.
    pub take_tracks: Vec<String>,
}

/// A change to one MIDI route.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MidiRouteRequest {
    Route { route: MidiRoute },
    Unroute { port: String, source: MidiSource },
}

/// Join the document with what is present into per-route reports.
///
/// Pure, so the sentence a user reads about a silent route is testable
/// without a synthesiser plugged in.
pub fn route_reports(
    routes: &[MidiRoute],
    outputs: &[MidiOutPortReport],
    inputs: &[MidiPortReport],
    take_tracks: &[String],
) -> Vec<MidiRouteReport> {
    routes
        .iter()
        .map(|route| {
            let port = outputs.iter().find(|p| p.name == route.port);
            let (status, reason) = match port {
                None => (
                    MidiRouteStatus::Missing,
                    Some(format!("“{}” is not connected", route.port)),
                ),
                Some(p) if p.absent => (
                    MidiRouteStatus::Missing,
                    Some(format!("“{}” is not connected", route.port)),
                ),
                Some(_) => match &route.source {
                    // An echo from a keyboard nobody plugged in sends
                    // nothing, and the port is not what is wrong.
                    MidiSource::Port { name }
                        if !inputs.iter().any(|p| &p.name == name && !p.absent) =>
                    {
                        (
                            MidiRouteStatus::Ready,
                            Some(format!("its input “{name}” is not connected")),
                        )
                    }
                    MidiSource::Take { name } if !take_tracks.iter().any(|t| t == name) => (
                        MidiRouteStatus::Ready,
                        Some(format!("the selected take has no MIDI track “{name}”")),
                    ),
                    _ => (MidiRouteStatus::Live, None),
                },
            };
            MidiRouteReport {
                port: route.port.clone(),
                status,
                reason,
            }
        })
        .collect()
}

async fn document(state: &AppState, midi: crate::instrument_host::MidiReport) -> MidiDto {
    let routes = state.control.snapshot().await.midi_routes;
    let take_tracks = state.control.take_midi_tracks().await;
    let reports = route_reports(&routes, &midi.outputs, &midi.inputs, &take_tracks);
    MidiDto {
        routes,
        reports,
        inputs: midi.inputs,
        outputs: midi.outputs,
        take_tracks,
    }
}

/// The MIDI patch bay document.
#[utoipa::path(
    get,
    path = "/api/v1/midi",
    responses((status = 200, description = "MIDI routes joined with port state", body = MidiDto))
)]
pub async fn list_midi(State(state): State<AppState>) -> Json<MidiDto> {
    let midi = state.instruments.report().await;
    Json(document(&state, midi).await)
}

/// Re-enumerate MIDI ports both ways and retry anything that failed.
#[utoipa::path(
    post,
    path = "/api/v1/midi/refresh",
    responses((status = 200, description = "Ports re-enumerated", body = MidiDto))
)]
pub async fn refresh_midi(State(state): State<AppState>) -> Json<MidiDto> {
    let midi = state.instruments.refresh().await;
    Json(document(&state, midi).await)
}

/// Add, change, or remove one MIDI route.
#[utoipa::path(
    put,
    path = "/api/v1/midi/routes",
    request_body = MidiRouteRequest,
    responses(
        (status = 200, description = "The MIDI patch bay after the change", body = MidiDto),
        (status = 404, description = "No such instrument, or no such route"),
        (status = 422, description = "Out of range or badly named"),
    )
)]
pub async fn route_midi(
    State(state): State<AppState>,
    Json(body): Json<MidiRouteRequest>,
) -> Result<Json<MidiDto>, ApiError> {
    let command = match body {
        MidiRouteRequest::Route { route } => MixCommand::SetMidiRoute { route },
        MidiRouteRequest::Unroute { port, source } => MixCommand::ClearMidiRoute { port, source },
    };
    state
        .control
        .apply(command, None)
        .await
        .map_err(map_mix_err)?;
    let midi = state.instruments.report().await;
    Ok(Json(document(&state, midi).await))
}

#[cfg(test)]
mod tests {
    use trib_core::InstrumentId;

    use super::*;

    fn out(name: &str, absent: bool) -> MidiOutPortReport {
        MidiOutPortReport {
            id: name.into(),
            name: name.into(),
            connected: !absent,
            absent,
            routes: 1,
            sent: 0,
            errors: 0,
        }
    }

    fn input(name: &str, absent: bool) -> MidiPortReport {
        MidiPortReport {
            id: name.into(),
            name: name.into(),
            connected: !absent,
            absent,
        }
    }

    fn route(port: &str, source: MidiSource) -> MidiRoute {
        MidiRoute {
            port: port.into(),
            channel: None,
            source,
        }
    }

    #[test]
    fn a_route_to_a_port_that_is_not_here_says_so() {
        let routes = vec![route(
            "Juno",
            MidiSource::Instrument {
                id: InstrumentId(0),
            },
        )];
        let reports = route_reports(&routes, &[out("Juno", true)], &[], &[]);
        assert_eq!(reports[0].status, MidiRouteStatus::Missing);
        assert!(
            reports[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("not connected")
        );
    }

    #[test]
    fn a_thru_whose_keyboard_is_unplugged_blames_the_keyboard_not_the_port() {
        // Both ends can be missing and they are different problems. Saying
        // "Juno is not connected" when the Juno is fine and the keyboard is
        // gone sends somebody to check the wrong cable.
        let routes = vec![route(
            "Juno",
            MidiSource::Port {
                name: "nanoKEY2".into(),
            },
        )];
        let reports = route_reports(
            &routes,
            &[out("Juno", false)],
            &[input("nanoKEY2", true)],
            &[],
        );
        assert_eq!(reports[0].status, MidiRouteStatus::Ready);
        assert!(reports[0].reason.as_ref().unwrap().contains("nanoKEY2"));
    }

    #[test]
    fn a_take_route_naming_a_track_this_take_lacks_says_which() {
        let routes = vec![route(
            "Juno",
            MidiSource::Take {
                name: "Kit Snare".into(),
            },
        )];
        let reports = route_reports(
            &routes,
            &[out("Juno", false)],
            &[],
            &["Kit Kick".to_owned()],
        );
        assert_eq!(reports[0].status, MidiRouteStatus::Ready);
        assert!(reports[0].reason.as_ref().unwrap().contains("Kit Snare"));
    }

    #[test]
    fn a_route_with_both_ends_present_is_live_and_says_nothing() {
        let routes = vec![route(
            "Juno",
            MidiSource::Port {
                name: "nanoKEY2".into(),
            },
        )];
        let reports = route_reports(
            &routes,
            &[out("Juno", false)],
            &[input("nanoKEY2", false)],
            &[],
        );
        assert_eq!(reports[0].status, MidiRouteStatus::Live);
        assert!(reports[0].reason.is_none());
    }
}
