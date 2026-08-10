//! The monitor pump: drains the engine's stream ring into framed binary
//! chunks for `/ws/monitor` listeners. Frames are self-describing
//! (generation + start_frame), so a client that missed some inserts
//! silence instead of drifting.
//!
//! Frame layout (little-endian, 24-byte header, 4-byte aligned):
//! `"TMON"` · `generation: u32` · `start_frame: u64` · `frame_count: u32`
//! · `flags: u32` (reserved) · `frame_count × 2 × f32` interleaved stereo.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use rtrb::Consumer;
use tokio::sync::broadcast;

/// Drain cadence — matches the meter pump.
const DRAIN_INTERVAL: Duration = Duration::from_millis(50);

/// Frames per chunk cap (~200 ms at 48 kHz): bounds message size when a
/// slow tick left the ring full.
const MAX_FRAMES_PER_CHUNK: usize = 9_600;

/// Encode one chunk. Pure — the wire shape lives here and in the test.
fn encode_frame(generation: u32, start_frame: u64, samples: &[f32]) -> Vec<u8> {
    let frame_count = (samples.len() / 2) as u32;
    let mut out = Vec::with_capacity(24 + samples.len() * 4);
    out.extend_from_slice(b"TMON");
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&start_frame.to_le_bytes());
    out.extend_from_slice(&frame_count.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags: reserved
    for &sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Fan the ring out to however many browser listeners subscribe. With no
/// listeners the ring still drains (and drops) so stale audio never bursts
/// out when one arrives.
pub fn spawn(
    mut monitor_rx: Consumer<f32>,
    generation: Arc<AtomicU32>,
    tx: broadcast::Sender<Bytes>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(DRAIN_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_generation = generation.load(Ordering::Relaxed);
        let mut sent_frames: u64 = 0;
        let mut samples: Vec<f32> = Vec::with_capacity(MAX_FRAMES_PER_CHUNK * 2);
        loop {
            interval.tick().await;
            let current = generation.load(Ordering::Relaxed);
            if current != last_generation {
                last_generation = current;
                sent_frames = 0;
            }
            let frames = (monitor_rx.slots() / 2).min(MAX_FRAMES_PER_CHUNK);
            if frames == 0 {
                continue;
            }
            samples.clear();
            for _ in 0..frames * 2 {
                match monitor_rx.pop() {
                    Ok(sample) => samples.push(sample),
                    Err(_) => break,
                }
            }
            if tx.receiver_count() > 0 {
                let frame = encode_frame(current, sent_frames, &samples);
                let _ = tx.send(Bytes::from(frame));
            }
            sent_frames += (samples.len() / 2) as u64;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_header_matches_the_documented_layout() {
        let frame = encode_frame(3, 12_000, &[0.5, -0.25]);
        assert_eq!(&frame[..4], b"TMON");
        assert_eq!(u32::from_le_bytes(frame[4..8].try_into().unwrap()), 3);
        assert_eq!(u64::from_le_bytes(frame[8..16].try_into().unwrap()), 12_000);
        assert_eq!(u32::from_le_bytes(frame[16..20].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(frame[20..24].try_into().unwrap()), 0);
        assert_eq!(f32::from_le_bytes(frame[24..28].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(frame[28..32].try_into().unwrap()), -0.25);
        assert_eq!(frame.len(), 32);
    }
}
