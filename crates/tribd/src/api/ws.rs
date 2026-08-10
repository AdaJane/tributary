use std::collections::HashSet;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use trib_core::{MeterKey, MixCommand, MixerState, StateDelta};
use utoipa::ToSchema;

use super::AppState;

/// Cap on distinct channels one socket may subscribe to. Only three channels
/// exist today; the cap bounds a hostile client's registry footprint.
const MAX_SUBSCRIPTIONS_PER_CLIENT: usize = 8;

/// Largest client frame we buffer. Protocol messages are tiny; anything
/// bigger is abuse.
const MAX_WS_MESSAGE_BYTES: usize = 16 * 1024;

/// A subscribable stream. Doubles as the routing key inside the hub.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Channel {
    /// The mixer document: a snapshot on subscribe, then state deltas.
    Mixer,
    /// Peak/clip meter batches at the pump rate (droppable).
    Meters,
    /// Recording state: take starts/stops, elapsed time.
    Transport,
    /// Live waveform bins while recording (droppable, Tracks-view only).
    Waveform,
}

/// Tags a `state_changed` broadcast with the client gesture that caused it,
/// so the issuing tab can reconcile its optimistic value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WsAck {
    pub client_id: String,
    pub seq: u64,
}

/// Messages the client sends over `/ws`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe {
        channel: Channel,
    },
    Unsubscribe {
        channel: Channel,
    },
    Ping,
    /// Continuous control gestures (fader/knob drags) ride WS, not REST —
    /// the one deliberate divergence from magma, justified by gesture rate.
    Set {
        command: MixCommand,
        seq: u64,
        client_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WsErrorCode {
    BadMessage,
    TooManySubscriptions,
    /// A `set` the reducer refused (unknown target, out-of-range value).
    Rejected,
}

/// One metered point in a `meters` batch.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct MeterDto {
    pub key: MeterKey,
    pub peak_db: f32,
    pub clip: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransportPhase {
    Stopped,
    Playing,
    Recording,
}

/// Where the playback mix goes: the console's hardware output, or the
/// browser monitor stream (M6). Hardware until the stream lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitorTarget {
    Hardware,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LoopRegionDto {
    pub start_frames: u64,
    pub end_frames: u64,
}

/// Playback gate for one lane of the latest take (index-aligned with the
/// take's tracks).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, ToSchema)]
pub struct LaneDto {
    pub solo: bool,
    pub mute: bool,
}

/// Transport state, pushed on the Transport channel on every state change
/// and served over REST. Position ticks ride `PlaybackPosition` instead.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct TransportDto {
    pub state: TransportPhase,
    /// Recording: the take being written. Otherwise: the latest take —
    /// what PLAY would roll.
    pub take: Option<u32>,
    /// Recording only.
    pub started_at_unix: Option<u64>,
    pub position_frames: u64,
    pub sample_rate: u32,
    /// Length of the latest take (its longest track).
    pub total_frames: u64,
    #[serde(rename = "loop")]
    pub loop_region: Option<LoopRegionDto>,
    pub monitor: MonitorTarget,
    pub lanes: Vec<LaneDto>,
}

/// Messages the daemon sends over `/ws`.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The full document — sent on every Mixer subscribe (and structural
    /// changes, from M3). Clients replace their mirror wholesale.
    MixerSnapshot {
        state: MixerState,
    },
    /// One applied mutation, broadcast to ALL clients (multi-UI sync).
    StateChanged {
        delta: StateDelta,
        ack: Option<WsAck>,
    },
    /// Coalesced meter batch at the pump rate. Droppable.
    Meters {
        frame: u64,
        meters: Vec<MeterDto>,
    },
    /// Transport state changed (play/stop/record/seek/gates).
    Transport {
        state: TransportDto,
    },
    /// Playhead progress at the pump rate while playing. Droppable; frames
    /// are take-timeline frames derived from real audio, not wall clock.
    PlaybackPosition {
        take: u32,
        frames: u64,
    },
    /// Freshly closed peak bins from the take writer — the live waveform.
    /// `bins` is interleaved (min, max) pairs scaled by 32767.
    WaveformBins {
        take: u32,
        track: u32,
        start_bin: u64,
        samples_per_bin: u32,
        bins: Vec<i16>,
    },
    Subscribed {
        channel: Channel,
    },
    Unsubscribed {
        channel: Channel,
    },
    Error {
        code: WsErrorCode,
        message: String,
    },
    Pong,
}

/// A subscription change the socket loop must mirror into the shared
/// registry. Returned rather than applied so the handler stays pure.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryDelta {
    Add(Channel),
    Remove(Channel),
}

/// Side effects the socket loop must perform for a handled message.
#[derive(Debug, PartialEq)]
pub enum WsAction {
    Registry(RegistryDelta),
    /// Forward to the control task; failures come back as `Rejected`.
    Apply {
        command: MixCommand,
        ack: WsAck,
    },
}

/// Pure request handling: mutates the per-client subscription set, returns
/// the immediate reply (if any) and the side effect (if any). The socket
/// loop performs both. No auth: the daemon binds loopback and the origin
/// check fences hostile pages — a deliberate posture for a locally-managed
/// tool.
pub fn handle_client_message(
    raw: &str,
    subs: &mut HashSet<Channel>,
) -> (Option<ServerMessage>, Option<WsAction>) {
    let message: ClientMessage = match serde_json::from_str(raw) {
        Ok(message) => message,
        Err(e) => {
            return (
                Some(ServerMessage::Error {
                    code: WsErrorCode::BadMessage,
                    message: e.to_string(),
                }),
                None,
            );
        }
    };
    match message {
        ClientMessage::Subscribe { channel } => {
            if subs.len() >= MAX_SUBSCRIPTIONS_PER_CLIENT && !subs.contains(&channel) {
                return (
                    Some(ServerMessage::Error {
                        code: WsErrorCode::TooManySubscriptions,
                        message: format!("limit is {MAX_SUBSCRIPTIONS_PER_CLIENT}"),
                    }),
                    None,
                );
            }
            let action = subs
                .insert(channel.clone())
                .then(|| WsAction::Registry(RegistryDelta::Add(channel.clone())));
            (Some(ServerMessage::Subscribed { channel }), action)
        }
        ClientMessage::Unsubscribe { channel } => {
            let action = subs
                .remove(&channel)
                .then(|| WsAction::Registry(RegistryDelta::Remove(channel.clone())));
            (Some(ServerMessage::Unsubscribed { channel }), action)
        }
        ClientMessage::Ping => (Some(ServerMessage::Pong), None),
        ClientMessage::Set {
            command,
            seq,
            client_id,
        } => (
            None,
            Some(WsAction::Apply {
                command,
                ack: WsAck { client_id, seq },
            }),
        ),
    }
}

/// Browsers do NOT apply CORS to the WebSocket handshake, so the origin
/// check is enforced here. Absent Origin (curl, native clients) passes —
/// the header only exists to fence browsers, and a hostile page always
/// presents its own domain. Loopback origins are trusted on ANY port
/// (Vite hops ports freely); everything else needs the configured list.
pub fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    match origin {
        None => true,
        Some(origin) => is_loopback_origin(origin) || allowed.iter().any(|a| a == origin),
    }
}

/// True for `http(s)://localhost[:port]`, `127.0.0.1`, or `[::1]`.
pub fn is_loopback_origin(origin: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // Bracketed IPv6: the port comes after the closing bracket.
        match rest.split_once(']') {
            Some((host, port)) if port.is_empty() || port.starts_with(':') => {
                return host == "::1";
            }
            _ => return false,
        }
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    matches!(host, "localhost" | "127.0.0.1")
}

pub async fn ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !origin_allowed(origin, &state.cors_origins) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    upgrade
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| client_loop(socket, state))
}

/// The monitor stream: one-way binary frames from the monitor pump (see
/// `monitor_pump` for the layout). Not the text hub — audio has its own
/// backpressure (lagged listeners skip; frames self-describe position).
pub async fn monitor_ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !origin_allowed(origin, &state.cors_origins) {
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    upgrade.on_upgrade(move |socket| async move {
        let mut frames = state.monitor_tx.subscribe();
        let (mut sink, mut stream) = socket.split();
        loop {
            tokio::select! {
                frame = frames.recv() => match frame {
                    Ok(bytes) => {
                        if sink.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    // Skipped frames surface as a start_frame gap; the
                    // client inserts silence rather than drifting.
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
                incoming = stream.next() => match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue, // one-way: ignore chatter
                },
            }
        }
    })
}

async fn send(sink: &mut SplitSink<WebSocket, Message>, message: &ServerMessage) -> bool {
    let json = serde_json::to_string(message).expect("ServerMessage serializes");
    sink.send(Message::Text(json.into())).await.is_ok()
}

/// One task per connected client: apply protocol messages, forward hub
/// events the client subscribed to.
async fn client_loop(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut hub_rx = state.hub.subscribe();
    let mut subs: HashSet<Channel> = HashSet::new();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue, // binary/ping/pong: nothing to do
                    Some(Err(_)) => break,
                };
                let (reply, action) =
                    handle_client_message(&text, &mut subs);
                if let Some(reply) = &reply
                    && !send(&mut sink, reply).await
                {
                    break;
                }
                match action {
                    Some(WsAction::Registry(delta)) => {
                        let subscribed_mixer = matches!(delta, RegistryDelta::Add(Channel::Mixer));
                        match delta {
                            RegistryDelta::Add(channel) => state.registry.add(&channel),
                            RegistryDelta::Remove(channel) => state.registry.remove(&channel),
                        }
                        // A Mixer subscriber immediately gets the document:
                        // REST snapshot + WS events collapse into one socket.
                        if subscribed_mixer {
                            let snapshot = ServerMessage::MixerSnapshot {
                                state: state.control.snapshot().await,
                            };
                            if !send(&mut sink, &snapshot).await {
                                break;
                            }
                        }
                    }
                    Some(WsAction::Apply { command, ack }) => {
                        if let Err(e) = state.control.apply(command, Some(ack)).await {
                            let refusal = ServerMessage::Error {
                                code: WsErrorCode::Rejected,
                                message: e.to_string(),
                            };
                            if !send(&mut sink, &refusal).await {
                                break;
                            }
                        }
                        // Success needs no direct reply: the state_changed
                        // broadcast (carrying the ack) reaches this client
                        // through its Mixer subscription.
                    }
                    None => {}
                }
            }
            broadcast = hub_rx.recv() => {
                match broadcast {
                    Ok((channel, payload)) => {
                        if subs.contains(&channel)
                            && sink.send(Message::Text(payload)).await.is_err()
                        {
                            break;
                        }
                    }
                    // A lagged client skips messages rather than dropping the
                    // connection: meters are droppable, and Mixer recovers on
                    // the next snapshot.
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "ws client lagged behind the hub");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    for channel in &subs {
        state.registry.remove(channel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trib_core::{FaderTarget, StripId};

    fn handle(raw: &str, subs: &mut HashSet<Channel>) -> (Option<ServerMessage>, Option<WsAction>) {
        handle_client_message(raw, subs)
    }

    #[test]
    fn subscribe_registers_and_replies() {
        let mut subs = HashSet::new();
        let (reply, action) = handle(
            r#"{"op":"subscribe","channel":{"kind":"meters"}}"#,
            &mut subs,
        );
        assert!(matches!(reply, Some(ServerMessage::Subscribed { .. })));
        assert_eq!(
            action,
            Some(WsAction::Registry(RegistryDelta::Add(Channel::Meters)))
        );
        assert!(subs.contains(&Channel::Meters));
    }

    #[test]
    fn duplicate_subscribe_does_not_double_register() {
        let mut subs = HashSet::new();
        let raw = r#"{"op":"subscribe","channel":{"kind":"mixer"}}"#;
        handle(raw, &mut subs);
        let (reply, action) = handle(raw, &mut subs);
        assert!(matches!(reply, Some(ServerMessage::Subscribed { .. })));
        assert_eq!(action, None, "second subscribe must not bump the refcount");
    }

    #[test]
    fn unsubscribe_removes_and_unknown_is_harmless() {
        let mut subs = HashSet::new();
        handle(
            r#"{"op":"subscribe","channel":{"kind":"transport"}}"#,
            &mut subs,
        );
        let (_, action) = handle(
            r#"{"op":"unsubscribe","channel":{"kind":"transport"}}"#,
            &mut subs,
        );
        assert_eq!(
            action,
            Some(WsAction::Registry(RegistryDelta::Remove(
                Channel::Transport
            )))
        );
        let (_, action) = handle(
            r#"{"op":"unsubscribe","channel":{"kind":"transport"}}"#,
            &mut subs,
        );
        assert_eq!(action, None);
    }

    #[test]
    fn bad_json_is_an_error_reply_not_a_disconnect() {
        let mut subs = HashSet::new();
        let (reply, action) = handle("not json", &mut subs);
        assert!(matches!(
            reply,
            Some(ServerMessage::Error {
                code: WsErrorCode::BadMessage,
                ..
            })
        ));
        assert_eq!(action, None);
    }

    #[test]
    fn ping_pongs() {
        let mut subs = HashSet::new();
        let (reply, _) = handle(r#"{"op":"ping"}"#, &mut subs);
        assert!(matches!(reply, Some(ServerMessage::Pong)));
    }

    #[test]
    fn a_set_becomes_an_apply_action_with_ack() {
        let mut subs = HashSet::new();
        let raw = r#"{"op":"set","command":{"op":"set_fader","target":{"kind":"strip","id":0},"level_db":-6.0},"seq":42,"client_id":"tab-1"}"#;
        let (reply, action) = handle(raw, &mut subs);
        assert_eq!(reply, None, "success replies ride the broadcast");
        let Some(WsAction::Apply { command, ack }) = action else {
            panic!("expected an apply action");
        };
        assert_eq!(
            command,
            MixCommand::SetFader {
                target: FaderTarget::Strip { id: StripId(0) },
                level_db: -6.0
            }
        );
        assert_eq!(ack.seq, 42);
        assert_eq!(ack.client_id, "tab-1");
    }

    #[test]
    fn server_messages_match_the_documented_wire_shape() {
        let subscribed = ServerMessage::Subscribed {
            channel: Channel::Meters,
        };
        assert_eq!(
            serde_json::to_string(&subscribed).unwrap(),
            r#"{"type":"subscribed","channel":{"kind":"meters"}}"#
        );
        let meters = ServerMessage::Meters {
            frame: 9,
            meters: vec![MeterDto {
                key: MeterKey::Master,
                peak_db: -12.5,
                clip: false,
            }],
        };
        assert_eq!(
            serde_json::to_string(&meters).unwrap(),
            r#"{"type":"meters","frame":9,"meters":[{"key":{"kind":"master"},"peak_db":-12.5,"clip":false}]}"#
        );
        let position = ServerMessage::PlaybackPosition {
            take: 2,
            frames: 96_000,
        };
        assert_eq!(
            serde_json::to_string(&position).unwrap(),
            r#"{"type":"playback_position","take":2,"frames":96000}"#
        );
        let bins = ServerMessage::WaveformBins {
            take: 4,
            track: 1,
            start_bin: 750,
            samples_per_bin: 512,
            bins: vec![-100, 200],
        };
        assert_eq!(
            serde_json::to_string(&bins).unwrap(),
            r#"{"type":"waveform_bins","take":4,"track":1,"start_bin":750,"samples_per_bin":512,"bins":[-100,200]}"#
        );
        let transport = ServerMessage::Transport {
            state: TransportDto {
                state: TransportPhase::Playing,
                take: Some(2),
                started_at_unix: None,
                position_frames: 480,
                sample_rate: 48_000,
                total_frames: 96_000,
                loop_region: Some(LoopRegionDto {
                    start_frames: 0,
                    end_frames: 4_800,
                }),
                monitor: MonitorTarget::Hardware,
                lanes: vec![LaneDto {
                    solo: true,
                    mute: false,
                }],
            },
        };
        assert_eq!(
            serde_json::to_string(&transport).unwrap(),
            r#"{"type":"transport","state":{"state":"playing","take":2,"started_at_unix":null,"position_frames":480,"sample_rate":48000,"total_frames":96000,"loop":{"start_frames":0,"end_frames":4800},"monitor":"hardware","lanes":[{"solo":true,"mute":false}]}}"#
        );
    }

    #[test]
    fn origin_check_fences_browsers_only() {
        let allowed = vec!["http://mixer.lan".to_string()];
        assert!(origin_allowed(None, &allowed), "non-browser clients pass");
        assert!(origin_allowed(Some("http://mixer.lan"), &allowed));
        assert!(!origin_allowed(Some("http://evil.example"), &allowed));
    }

    #[test]
    fn loopback_origins_pass_on_any_port() {
        // Vite hops ports (5173 taken → 5174 → 5175); localhost pages are
        // the operator's own machine, so no port pinning.
        for origin in [
            "http://localhost:5173",
            "http://localhost:5175",
            "http://127.0.0.1:9999",
            "http://localhost",
            "https://localhost:8443",
            "http://[::1]:5173",
        ] {
            assert!(origin_allowed(Some(origin), &[]), "{origin} must pass");
        }
        for origin in [
            "http://evil.example",
            "http://localhost.evil.example:5173",
            "http://127.0.0.1.evil.example",
            "ftp://localhost",
        ] {
            assert!(!origin_allowed(Some(origin), &[]), "{origin} must fail");
        }
    }
}
