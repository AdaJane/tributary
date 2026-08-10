use axum::extract::ws::Utf8Bytes;
use tokio::sync::broadcast;

use crate::api::ws::{Channel, ServerMessage};

/// Fan-out hub between the engine host / meter pump and WebSocket clients.
///
/// One broadcast stream carrying `(channel, pre-serialized payload)`; each
/// client task filters against its own subscription set. Payloads are
/// serialized once and shared as cheap `Utf8Bytes` clones. If per-client
/// filtering ever shows up in profiles, upgrade to one broadcast sender per
/// channel behind the same `publish` API.
#[derive(Clone)]
pub struct Hub {
    tx: broadcast::Sender<(Channel, Utf8Bytes)>,
}

impl Hub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Hub { tx }
    }

    /// Serialize once and broadcast to all connected clients. No subscribers
    /// is normal (returns silently); serialization failure is a bug in our
    /// own types, so it panics.
    pub fn publish(&self, channel: Channel, message: &ServerMessage) {
        let json = serde_json::to_string(message).expect("ServerMessage serializes");
        let _ = self.tx.send((channel, json.into()));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(Channel, Utf8Bytes)> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_reaches_subscribers_with_channel_tag() {
        let hub = Hub::new(16);
        let mut rx = hub.subscribe();
        hub.publish(Channel::Meters, &ServerMessage::Pong);

        let (channel, payload) = rx.recv().await.unwrap();
        assert_eq!(channel, Channel::Meters);
        assert_eq!(payload.as_str(), r#"{"type":"pong"}"#);
    }

    #[test]
    fn publish_without_subscribers_is_fine() {
        Hub::new(16).publish(Channel::Mixer, &ServerMessage::Pong);
    }
}
