use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use trib_core::{MixCommand, OutputJack, OutputPatch, SendTap};
use utoipa::ToSchema;

use super::{ApiError, AppState};
use crate::api::strips::map_mix_err;
use crate::device_host::OutputDeviceReport;

/// Why one patch is or is not carrying signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputPatchStatus {
    /// Wired to an open device, and the source is not silencing it.
    Live,
    /// Wired, but something upstream means nothing will come out.
    Ready,
    /// The device it names is not here.
    Missing,
    /// The device is here and would not open.
    Failed,
}

/// One patch, joined with what the daemon knows about where it goes.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OutputPatchReport {
    pub device: Option<String>,
    pub channel: u16,
    pub status: OutputPatchStatus,
    /// The daemon's own sentence, in the console's voice. `None` when
    /// there is nothing to explain.
    pub reason: Option<String>,
}

/// The whole output patch bay in one read — the document half and the
/// machine half together.
///
/// One document for the reason `InstrumentsDto` is one document: pulling
/// an interface changes a patch's status, and split reads would let the
/// console render a pair that never existed together.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OutputsDto {
    pub patches: Vec<OutputPatch>,
    /// Index-aligned with `patches`.
    pub reports: Vec<OutputPatchReport>,
    pub devices: Vec<OutputDeviceReport>,
    /// Whether this backend can drive patchable outputs at all. False
    /// makes every mutation a 501 rather than a silent no-op.
    pub supported: bool,
}

/// A change to one output channel.
///
/// A tagged envelope rather than a nullable field, because the identity
/// here is TWO values: a body naming a patch and saying `null` in the same
/// breath would be representable, and would have to be refused at runtime.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OutputPatchRequest {
    Patch { patch: OutputPatch },
    Unpatch { jack: OutputJack },
    Tap { jack: OutputJack, tap: SendTap },
}

/// Split a request into the commands it implies, against the jack as it
/// currently stands.
///
/// Pure, so the split — which decides what the tape refuses — is testable
/// without a running daemon. A request that moves only the tap becomes a
/// `SetOutputTap`, which stays live under the tape; one that changes
/// nothing becomes nothing, so a console that PUTs its whole form on every
/// keystroke cannot stop a take by writing back the source it already has.
pub fn patch_commands(current: Option<&OutputPatch>, req: OutputPatchRequest) -> Vec<MixCommand> {
    match req {
        OutputPatchRequest::Patch { patch } => match current {
            Some(now) if *now == patch => Vec::new(),
            Some(now)
                if now.source == patch.source && now.source_channel == patch.source_channel =>
            {
                vec![MixCommand::SetOutputTap {
                    jack: patch.jack(),
                    tap: patch.tap,
                }]
            }
            _ => vec![MixCommand::SetOutputPatch { patch }],
        },
        OutputPatchRequest::Unpatch { jack } => vec![MixCommand::ClearOutputPatch { jack }],
        OutputPatchRequest::Tap { jack, tap } => match current {
            Some(now) if now.tap == tap => Vec::new(),
            _ => vec![MixCommand::SetOutputTap { jack, tap }],
        },
    }
}

/// Join the document with the device list into per-patch reports.
///
/// Pure for the same reason `silencedReason` is pure on the console side:
/// the sentence a user reads about a dead output is worth testing without
/// hardware attached.
pub fn patch_reports(
    patches: &[OutputPatch],
    devices: &[OutputDeviceReport],
) -> Vec<OutputPatchReport> {
    patches
        .iter()
        .map(|patch| {
            let device = devices.iter().find(|d| match &patch.device {
                None => d.active,
                Some(name) => &d.name == name,
            });
            let name = patch
                .device
                .clone()
                .unwrap_or_else(|| "the default output".to_owned());
            let (status, reason) = match device {
                None => (
                    OutputPatchStatus::Missing,
                    Some(format!("its output “{name}” is not connected")),
                ),
                Some(d) if d.error.is_some() => (OutputPatchStatus::Failed, d.error.clone()),
                Some(d) if patch.channel >= d.channels.saturating_sub(d.monitor_channels) => (
                    OutputPatchStatus::Missing,
                    Some(format!(
                        "its output “{name}” has only {} patchable channels",
                        d.channels.saturating_sub(d.monitor_channels)
                    )),
                ),
                Some(d) if d.muted => (
                    OutputPatchStatus::Failed,
                    Some(format!("the output “{name}” is muted")),
                ),
                Some(d) if d.volume_percent == Some(0) => (
                    OutputPatchStatus::Failed,
                    Some(format!("the output “{name}” is turned down to zero")),
                ),
                Some(d) if d.status != crate::device_host::DeviceStatus::Open => {
                    (OutputPatchStatus::Ready, None)
                }
                Some(_) => (OutputPatchStatus::Live, None),
            };
            OutputPatchReport {
                device: patch.device.clone(),
                channel: patch.channel,
                status,
                reason,
            }
        })
        .collect()
}

async fn document(state: &AppState, devices: Vec<OutputDeviceReport>) -> OutputsDto {
    let patches = state.control.snapshot().await.outputs;
    let reports = patch_reports(&patches, &devices);
    OutputsDto {
        patches,
        reports,
        devices,
        supported: state.supports_outputs,
    }
}

/// The output patch bay document: every output device the OS reports,
/// joined with what the daemon has open and what the console has patched.
#[utoipa::path(
    get,
    path = "/api/v1/outputs",
    responses((status = 200, description = "Output devices joined with patch state", body = OutputsDto))
)]
pub async fn list_outputs(State(state): State<AppState>) -> Json<OutputsDto> {
    let devices = state.devices.list_outputs().await;
    Json(document(&state, devices).await)
}

/// The Refresh button: re-enumerate, retry every wanted-but-unopened or
/// failed output, and return the fresh document.
#[utoipa::path(
    post,
    path = "/api/v1/outputs/refresh",
    responses((status = 200, description = "Outputs re-enumerated and reconciled", body = OutputsDto))
)]
pub async fn refresh_outputs(State(state): State<AppState>) -> Json<OutputsDto> {
    let devices = state.devices.refresh_outputs().await;
    Json(document(&state, devices).await)
}

/// Patch, unpatch, or re-tap one output channel.
#[utoipa::path(
    put,
    path = "/api/v1/outputs/patch",
    request_body = OutputPatchRequest,
    responses(
        (status = 200, description = "The patch bay after the change", body = OutputsDto),
        (status = 404, description = "No such source, or no patch on that jack"),
        (status = 409, description = "Refused while recording"),
        (status = 422, description = "Out of range"),
        (status = 501, description = "This backend has no patchable outputs"),
    )
)]
pub async fn patch_output(
    State(state): State<AppState>,
    Json(body): Json<OutputPatchRequest>,
) -> Result<Json<OutputsDto>, ApiError> {
    if !state.supports_outputs {
        return Err(ApiError::Unsupported(
            "this audio backend has no patchable outputs".into(),
        ));
    }
    let jack = match &body {
        OutputPatchRequest::Patch { patch } => patch.jack(),
        OutputPatchRequest::Unpatch { jack } | OutputPatchRequest::Tap { jack, .. } => jack.clone(),
    };
    let current = state.control.snapshot().await.output(&jack).cloned();
    for command in patch_commands(current.as_ref(), body) {
        state
            .control
            .apply(command, None)
            .await
            .map_err(map_mix_err)?;
    }
    let devices = state.devices.list_outputs().await;
    Ok(Json(document(&state, devices).await))
}

#[cfg(test)]
mod tests {
    use trib_core::{OutputSource, StripId};

    use super::*;
    use crate::device_host::DeviceStatus;

    fn jack(channel: u16) -> OutputJack {
        OutputJack {
            device: Some("interface".into()),
            channel,
        }
    }

    fn patch(channel: u16, tap: SendTap) -> OutputPatch {
        let mut patch = OutputPatch::new(OutputSource::Master, 0, jack(channel));
        patch.tap = tap;
        patch
    }

    fn device(channels: u16, monitor_channels: u16) -> OutputDeviceReport {
        OutputDeviceReport {
            name: "interface".into(),
            label: None,
            channels,
            monitor_channels,
            active: false,
            status: DeviceStatus::Open,
            patched: true,
            underruns: 0,
            overruns: 0,
            xruns: 0,
            worst_block_us: 0,
            muted: false,
            volume_percent: None,
            error: None,
        }
    }

    #[test]
    fn writing_back_the_patch_that_is_already_there_costs_nothing() {
        // The bug this prevents: a console that PUTs its whole form on
        // every keystroke would stop the tape on every keystroke.
        let now = patch(3, SendTap::PreFader);
        assert!(
            patch_commands(Some(&now), OutputPatchRequest::Patch { patch: now.clone() }).is_empty()
        );
        assert!(
            patch_commands(
                Some(&now),
                OutputPatchRequest::Tap {
                    jack: jack(3),
                    tap: SendTap::PreFader,
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn moving_only_the_tap_becomes_the_command_the_tape_allows() {
        let now = patch(3, SendTap::PreFader);
        let moved = patch(3, SendTap::PostFader);
        assert_eq!(
            patch_commands(Some(&now), OutputPatchRequest::Patch { patch: moved }),
            vec![MixCommand::SetOutputTap {
                jack: jack(3),
                tap: SendTap::PostFader
            }],
            "same source, different tap: a flag write, not a re-wire"
        );
    }

    #[test]
    fn changing_the_source_is_a_re_wire() {
        let now = patch(3, SendTap::PreFader);
        let mut other = patch(3, SendTap::PreFader);
        other.source = OutputSource::Strip { id: StripId(1) };
        assert!(matches!(
            patch_commands(Some(&now), OutputPatchRequest::Patch { patch: other })
                .first()
                .unwrap(),
            MixCommand::SetOutputPatch { .. }
        ));
    }

    #[test]
    fn a_patch_past_the_devices_patchable_channels_says_so() {
        // The monitor's channels are not offered, so a device with 4
        // channels and 2 taken has 2 to give — and a patch at 2 is one too
        // far, however healthy the device looks.
        let patches = vec![patch(2, SendTap::PreFader)];
        let reports = patch_reports(&patches, &[device(4, 2)]);
        assert_eq!(reports[0].status, OutputPatchStatus::Missing);
        assert!(reports[0].reason.as_ref().unwrap().contains("only 2"));
    }

    #[test]
    fn a_muted_output_is_explained_rather_than_called_healthy() {
        // An output has no meter. If the report does not say this, a dead
        // PA has no explanation anywhere in the console.
        let patches = vec![patch(0, SendTap::PreFader)];
        let mut muted = device(8, 0);
        muted.muted = true;
        let reports = patch_reports(&patches, &[muted]);
        assert_eq!(reports[0].status, OutputPatchStatus::Failed);
        assert!(reports[0].reason.as_ref().unwrap().contains("muted"));
    }

    #[test]
    fn a_patch_naming_a_device_that_is_not_here_says_that_instead() {
        let patches = vec![patch(0, SendTap::PreFader)];
        let reports = patch_reports(&patches, &[]);
        assert_eq!(reports[0].status, OutputPatchStatus::Missing);
        assert!(
            reports[0]
                .reason
                .as_ref()
                .unwrap()
                .contains("is not connected")
        );
    }
}
