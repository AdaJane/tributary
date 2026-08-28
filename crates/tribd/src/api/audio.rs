//! Is the audio backend making sound at all — and if not, why.
//!
//! Every other document (`/devices`, `/outputs`) is a list of what the
//! backend can see. When the exclusive layer's card never opened, those
//! lists are honestly EMPTY, and an empty list looks exactly like "nothing
//! plugged in". This is the one read that distinguishes the two, and it is
//! served from the same struct the audio thread writes its journal line
//! from, so the console and the log cannot disagree.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use trib_audio::{BackendStatus, RealtimeStatus, Scheduling};
use utoipa::ToSchema;

use super::AppState;
use crate::settings::AudioLayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingDto {
    Fifo,
    Other,
    NotApplicable,
}

impl From<Scheduling> for SchedulingDto {
    fn from(value: Scheduling) -> Self {
        match value {
            Scheduling::Fifo => SchedulingDto::Fifo,
            Scheduling::Other => SchedulingDto::Other,
            Scheduling::NotApplicable => SchedulingDto::NotApplicable,
        }
    }
}

/// What the machine granted the audio thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct RealtimeDto {
    pub scheduling: SchedulingDto,
    pub priority: Option<u32>,
    pub memory_locked: bool,
    /// Why it is not better than this, in a sentence someone can act on.
    pub reason: Option<String>,
}

impl From<RealtimeStatus> for RealtimeDto {
    fn from(value: RealtimeStatus) -> Self {
        RealtimeDto {
            scheduling: value.scheduling.into(),
            priority: value.priority,
            memory_locked: value.memory_locked,
            reason: value.reason,
        }
    }
}

/// The audio backend's own state, apart from any device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AudioStatusDto {
    /// Which device layer the daemon was configured to run.
    pub layer: AudioLayer,
    /// The backend's own name for itself.
    pub backend: String,
    /// Whether the backend's thread came up at boot at all. False is a
    /// daemon-level failure that only a restart changes.
    pub started: bool,
    /// Whether it is making sound right now. The exclusive layer is
    /// running only while its card is open; it keeps retrying while not.
    pub running: bool,
    /// The card the exclusive layer holds, when it holds one.
    pub card: Option<String>,
    /// Why it is not running, when it is not.
    pub error: Option<String>,
    pub realtime: RealtimeDto,
}

/// Fold boot and backend state into one document. Pure, so the table of
/// answers is tested without a backend.
pub fn audio_status(
    layer: AudioLayer,
    backend: &str,
    started: bool,
    status: BackendStatus,
) -> AudioStatusDto {
    AudioStatusDto {
        layer,
        backend: backend.to_owned(),
        started,
        // A backend that never started is not running whatever it claims:
        // its status is the default the trait hands back, not a report.
        running: started && status.running,
        card: status.card.filter(|_| started),
        error: if started {
            status.error
        } else {
            Some("the audio backend failed to start; restart the daemon".to_owned())
        },
        realtime: status.realtime.into(),
    }
}

/// The audio backend's state: started, running, which card, and why not.
#[utoipa::path(
    get,
    path = "/api/v1/audio",
    responses((status = 200, description = "Audio backend state", body = AudioStatusDto))
)]
pub async fn get_audio(State(state): State<AppState>) -> Json<AudioStatusDto> {
    Json(audio_status(
        state.audio_layer,
        state.audio.name(),
        state.audio_started,
        state.audio.status(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realtime() -> RealtimeStatus {
        RealtimeStatus {
            scheduling: Scheduling::Fifo,
            priority: Some(10),
            memory_locked: true,
            reason: None,
        }
    }

    #[test]
    fn a_backend_that_never_started_is_not_running_whatever_it_says() {
        let dto = audio_status(
            AudioLayer::Exclusive,
            "alsa",
            false,
            BackendStatus {
                running: true,
                card: Some("hw:0".into()),
                error: None,
                realtime: RealtimeStatus::not_applicable(),
            },
        );
        assert!(!dto.running);
        assert_eq!(
            dto.card, None,
            "a card claimed by a thread that never ran is not held"
        );
        assert!(dto.error.as_deref().is_some_and(|e| e.contains("restart")));
        assert!(!dto.started);
    }

    #[test]
    fn a_started_backend_waiting_for_its_card_reports_the_open_error() {
        let dto = audio_status(
            AudioLayer::Exclusive,
            "alsa",
            true,
            BackendStatus {
                running: false,
                card: None,
                error: Some("hw:0 (Capture): No such file or directory".into()),
                realtime: realtime(),
            },
        );
        assert!(dto.started && !dto.running);
        assert_eq!(
            dto.error.as_deref(),
            Some("hw:0 (Capture): No such file or directory")
        );
        assert_eq!(dto.realtime.scheduling, SchedulingDto::Fifo);
        assert_eq!(dto.realtime.priority, Some(10));
    }

    #[test]
    fn a_running_backend_names_its_card_and_carries_no_error() {
        let dto = audio_status(
            AudioLayer::Exclusive,
            "alsa",
            true,
            BackendStatus {
                running: true,
                card: Some("hw:1".into()),
                error: None,
                realtime: realtime(),
            },
        );
        assert_eq!(
            dto,
            AudioStatusDto {
                layer: AudioLayer::Exclusive,
                backend: "alsa".into(),
                started: true,
                running: true,
                card: Some("hw:1".into()),
                error: None,
                realtime: RealtimeDto {
                    scheduling: SchedulingDto::Fifo,
                    priority: Some(10),
                    memory_locked: true,
                    reason: None,
                },
            }
        );
    }

    #[test]
    fn the_shared_layer_is_simply_running() {
        let backend: &dyn trib_audio::AudioBackend = &trib_audio::FakeBackend::default();
        let dto = audio_status(AudioLayer::Shared, backend.name(), true, backend.status());
        assert!(dto.running);
        assert_eq!(dto.card, None);
        assert_eq!(dto.error, None);
        assert_eq!(dto.realtime.scheduling, SchedulingDto::NotApplicable);
    }
}
