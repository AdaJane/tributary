use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::api::ws::Channel;

/// Refcounts of live client subscriptions per channel. Producers that cost
/// something per tick (the meter pump) publish only when someone is watching.
/// Sync RwLock: critical sections are pure map ops, never held across an
/// await.
#[derive(Clone, Default)]
pub struct ChannelRegistry {
    inner: Arc<RwLock<HashMap<Channel, usize>>>,
}

impl ChannelRegistry {
    pub fn add(&self, channel: &Channel) {
        let mut map = self.inner.write().unwrap_or_else(|e| e.into_inner());
        *map.entry(channel.clone()).or_insert(0) += 1;
    }

    pub fn remove(&self, channel: &Channel) {
        let mut map = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = map.get_mut(channel) {
            *count -= 1;
            if *count == 0 {
                map.remove(channel);
            }
        }
    }

    /// Whether at least one client is subscribed to `channel`.
    pub fn is_watched(&self, channel: &Channel) -> bool {
        let map = self.inner.read().unwrap_or_else(|e| e.into_inner());
        map.contains_key(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refcounts_track_add_remove() {
        let registry = ChannelRegistry::default();
        registry.add(&Channel::Meters);
        registry.add(&Channel::Meters);
        registry.remove(&Channel::Meters);
        assert!(registry.is_watched(&Channel::Meters), "one subscriber left");
        registry.remove(&Channel::Meters);
        assert!(!registry.is_watched(&Channel::Meters));
    }

    #[test]
    fn removing_unknown_channel_is_harmless() {
        let registry = ChannelRegistry::default();
        registry.remove(&Channel::Transport);
        assert!(!registry.is_watched(&Channel::Transport));
    }
}
