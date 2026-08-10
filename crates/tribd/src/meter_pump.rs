//! Drains engine telemetry and publishes coalesced meter batches to the
//! hub. One WS message per tick regardless of meter count.

use std::time::Duration;

use rtrb::Consumer;
use tokio::sync::watch;
use trib_core::{MeterKey, linear_to_db};
use trib_engine::{MeterBlock, Retired};

use crate::api::ws::{Channel, MeterDto, ServerMessage};
use crate::hub::Hub;
use crate::registry::ChannelRegistry;

/// 20 Hz: fast enough for LEDs to read as live, one message per tick cheap
/// enough for a full console of subscribed clients.
const METER_INTERVAL: Duration = Duration::from_millis(50);

/// `keys[i]` names metered point `i` of the engine's `MeterBlock` layout;
/// the control task republishes the mapping on every recompile. The pump
/// also drops retirees (graphs, finished record sets) — deallocation is
/// control-side by contract, and dropping a record set's producers is what
/// lets the take writer finalize.
pub fn spawn(
    hub: Hub,
    registry: ChannelRegistry,
    meter_rx: Consumer<MeterBlock>,
    retire_rx: Consumer<Retired>,
    keys: watch::Receiver<Vec<MeterKey>>,
) {
    tokio::spawn(pump_loop(hub, registry, meter_rx, retire_rx, keys));
}

async fn pump_loop(
    hub: Hub,
    registry: ChannelRegistry,
    mut meter_rx: Consumer<MeterBlock>,
    mut retire_rx: Consumer<Retired>,
    keys: watch::Receiver<Vec<MeterKey>>,
) {
    let mut ticker = tokio::time::interval(METER_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        // Retirees die here, off the audio thread.
        while let Ok(retired) = retire_rx.pop() {
            drop(retired);
        }
        // Always drain — even unwatched — so the ring never backs up.
        let Some(block) = drain_coalescing(&mut meter_rx) else {
            continue;
        };
        if !registry.is_watched(&Channel::Meters) {
            continue;
        }
        let keys = keys.borrow();
        let meters = (0..block.count as usize)
            .filter_map(|i| {
                keys.get(i).map(|&key| MeterDto {
                    key,
                    peak_db: linear_to_db(block.peak[i]),
                    clip: block.clipped(i),
                })
            })
            .collect();
        drop(keys);
        hub.publish(
            Channel::Meters,
            &ServerMessage::Meters {
                frame: block.frame,
                meters,
            },
        );
    }
}

/// Max of peaks, OR of clips, latest frame — a tick's worth of blocks
/// becomes one reading, so a slow tick never understates a transient.
fn drain_coalescing(rx: &mut Consumer<MeterBlock>) -> Option<MeterBlock> {
    let mut coalesced: Option<MeterBlock> = None;
    while let Ok(block) = rx.pop() {
        coalesced = Some(match coalesced {
            None => block,
            Some(mut acc) => {
                for i in 0..block.count as usize {
                    acc.peak[i] = acc.peak[i].max(block.peak[i]);
                }
                acc.clip_bits |= block.clip_bits;
                acc.frame = block.frame;
                acc.count = acc.count.max(block.count);
                acc
            }
        });
    }
    coalesced
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing_keeps_the_loudest_peak_and_any_clip() {
        let (mut tx, mut rx) = rtrb::RingBuffer::new(8);
        let mut a = MeterBlock::empty(1);
        a.count = 1;
        a.peak[0] = 0.9;
        a.clip_bits = 1;
        let mut b = MeterBlock::empty(2);
        b.count = 1;
        b.peak[0] = 0.3;
        tx.push(a).unwrap();
        tx.push(b).unwrap();

        let out = drain_coalescing(&mut rx).unwrap();
        assert_eq!(
            out.peak[0], 0.9,
            "quiet later block must not mask a transient"
        );
        assert!(out.clipped(0), "clip survives coalescing");
        assert_eq!(out.frame, 2, "frame counter follows the latest block");
        assert!(drain_coalescing(&mut rx).is_none(), "ring fully drained");
    }
}
