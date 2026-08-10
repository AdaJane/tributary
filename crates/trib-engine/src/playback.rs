//! Playback: the tape return. A feeder thread (trib-project) fills
//! per-track rings from a take's WAVs; the audio thread pops frames in
//! lockstep, gates lanes for solo/mute, and mixes to stereo. Built
//! control-side, moved in via `StartPlayback`, retired on stop or finish.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rtrb::Consumer;
use trib_dsp::SmoothedParam;

/// Constant-power center for mono lanes folded to stereo — the same −3 dB
/// law the console's pan uses.
const CENTER: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// Position/finished flags shared with the control side. Relaxed stores on
/// the audio thread; the 20 Hz ticker and REST reads are the consumers.
#[derive(Debug, Default)]
pub struct PlaybackShared {
    /// Frames actually popped since the session started — real audio
    /// progress, never wall clock.
    pub position: AtomicU64,
    pub finished: AtomicBool,
}

pub struct PlaybackTrack {
    pub rx: Consumer<f32>,
    /// 1 = mono strip track, 2 = interleaved stereo. Only the master track
    /// is stereo in v1.
    pub channels: u16,
    pub is_master: bool,
    pub solo: bool,
    pub mute: bool,
    /// Audibility gate, ramped so solo/mute flips never click.
    pub gate: SmoothedParam,
}

/// The program rule: with no solo you hear the recorded master (the mix the
/// room heard); solo isolates dry lanes; a take without a master track
/// falls back to a unity sum of unmuted lanes.
pub fn lane_audible(has_master: bool, any_solo: bool, track: &PlaybackTrack) -> bool {
    if track.mute {
        return false;
    }
    if any_solo {
        return track.solo;
    }
    if has_master {
        return track.is_master;
    }
    true
}

pub struct PlaybackSet {
    pub tracks: Vec<PlaybackTrack>,
    pub has_master: bool,
    pub shared: Arc<PlaybackShared>,
}

impl PlaybackSet {
    /// Gates start closed and ramp toward the program rule, so a session
    /// fades in instead of clicking mid-take.
    pub fn new(tracks: Vec<PlaybackTrack>, has_master: bool) -> Self {
        let mut set = PlaybackSet {
            tracks,
            has_master,
            shared: Arc::new(PlaybackShared::default()),
        };
        set.retarget_gates();
        set
    }

    /// Stale track indices (a seek swapped the session) are ignored, not
    /// errors — same doctrine as stale `ParamIx` after a graph swap.
    pub fn set_solo(&mut self, track: u16, on: bool) {
        if let Some(t) = self.tracks.get_mut(track as usize) {
            t.solo = on;
            self.retarget_gates();
        }
    }

    pub fn set_mute(&mut self, track: u16, on: bool) {
        if let Some(t) = self.tracks.get_mut(track as usize) {
            t.mute = on;
            self.retarget_gates();
        }
    }

    /// One flip can change every lane's audibility (soloing swaps the
    /// program off the master), so all gates retarget together.
    fn retarget_gates(&mut self) {
        let any_solo = self.tracks.iter().any(|t| t.solo);
        let has_master = self.has_master;
        for track in &mut self.tracks {
            let on = lane_audible(has_master, any_solo, track);
            track.gate.set_target(if on { 1.0 } else { 0.0 });
        }
    }

    /// Overwrite `output` (interleaved stereo) with the playback mix.
    /// Returns frames advanced. A stalled feeder fills the shortfall with
    /// silence and does not advance — lanes stay aligned by construction
    /// because every live ring pops the same frame count.
    pub fn mix_chunk(&mut self, output: &mut [f32]) -> usize {
        let frames = output.len() / 2;
        let mut avail = frames;
        let mut all_done = true;
        for track in &self.tracks {
            if track.rx.is_abandoned() && track.rx.is_empty() {
                continue; // exhausted early: contributes silence, not a stall
            }
            all_done = false;
            avail = avail.min(track.rx.slots() / track.channels as usize);
        }
        if all_done {
            // Output untouched: the live mix returns the same block the
            // tape runs out, with no gap of forced silence.
            self.shared.finished.store(true, Ordering::Relaxed);
            return 0;
        }
        output.fill(0.0);
        for track in &mut self.tracks {
            for i in 0..avail {
                let gain = track.gate.tick();
                if track.channels == 2 {
                    let l = track.rx.pop().unwrap_or(0.0);
                    let r = track.rx.pop().unwrap_or(0.0);
                    output[i * 2] += l * gain;
                    output[i * 2 + 1] += r * gain;
                } else {
                    let s = track.rx.pop().unwrap_or(0.0) * CENTER * gain;
                    output[i * 2] += s;
                    output[i * 2 + 1] += s;
                }
            }
        }
        avail
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    const SR: u32 = 48_000;
    /// Two 256-frame chunks comfortably outrun the 10 ms gate ramp.
    const SETTLE_CHUNKS: usize = 3;

    fn track(rx: Consumer<f32>, channels: u16, is_master: bool) -> PlaybackTrack {
        PlaybackTrack {
            rx,
            channels,
            is_master,
            solo: false,
            mute: false,
            gate: SmoothedParam::new(0.0, SR),
        }
    }

    fn filled(frames: usize, channels: u16, value: f32) -> (rtrb::Producer<f32>, Consumer<f32>) {
        let (mut tx, rx) = RingBuffer::new(frames * channels as usize);
        for _ in 0..frames * channels as usize {
            tx.push(value).unwrap();
        }
        (tx, rx)
    }

    fn settle(set: &mut PlaybackSet) {
        let mut sink = vec![0.0; 256 * 2];
        for _ in 0..SETTLE_CHUNKS {
            set.mix_chunk(&mut sink);
        }
    }

    #[test]
    fn the_program_rule_prefers_master_then_solo_then_unity_sum() {
        let lane = |is_master, solo, mute| PlaybackTrack {
            rx: RingBuffer::new(1).1,
            channels: 1,
            is_master,
            solo,
            mute,
            gate: SmoothedParam::new(0.0, SR),
        };
        // No solo, master present: only the master lane sounds.
        assert!(lane_audible(true, false, &lane(true, false, false)));
        assert!(!lane_audible(true, false, &lane(false, false, false)));
        // Any solo: exactly the soloed, unmuted lanes sound.
        assert!(lane_audible(true, true, &lane(false, true, false)));
        assert!(!lane_audible(true, true, &lane(true, false, false)));
        assert!(!lane_audible(true, true, &lane(false, true, true)));
        // No master track: unity sum of unmuted lanes.
        assert!(lane_audible(false, false, &lane(false, false, false)));
        assert!(!lane_audible(false, false, &lane(false, false, true)));
        // Mute always removes the lane.
        assert!(!lane_audible(true, false, &lane(true, false, true)));
    }

    #[test]
    fn with_a_master_track_only_the_master_reaches_the_output() {
        let (_mtx, master_rx) = filled(2048, 2, 0.5);
        let (_stx, strip_rx) = filled(2048, 1, 0.9);
        let mut set = PlaybackSet::new(
            vec![track(master_rx, 2, true), track(strip_rx, 1, false)],
            true,
        );
        settle(&mut set);
        let mut out = vec![0.0; 256 * 2];
        let popped = set.mix_chunk(&mut out);
        assert_eq!(popped, 256);
        assert!(
            (out[0] - 0.5).abs() < 1e-6,
            "master L verbatim, got {}",
            out[0]
        );
        assert!((out[1] - 0.5).abs() < 1e-6, "master R verbatim");
    }

    #[test]
    fn soloing_a_lane_swaps_the_program_off_the_master() {
        let (_mtx, master_rx) = filled(4096, 2, 0.5);
        let (_stx, strip_rx) = filled(4096, 1, 0.8);
        let mut set = PlaybackSet::new(
            vec![track(master_rx, 2, true), track(strip_rx, 1, false)],
            true,
        );
        set.set_solo(1, true);
        settle(&mut set);
        let mut out = vec![0.0; 256 * 2];
        set.mix_chunk(&mut out);
        let expected = 0.8 * core::f32::consts::FRAC_1_SQRT_2;
        assert!((out[0] - expected).abs() < 1e-6, "dry lane at −3 dB center");
        assert!((out[1] - expected).abs() < 1e-6, "same on both sides");
    }

    #[test]
    fn without_a_master_track_unmuted_lanes_sum_at_unity() {
        let (_atx, a_rx) = filled(2048, 1, 0.2);
        let (_btx, b_rx) = filled(2048, 1, 0.3);
        let mut set = PlaybackSet::new(vec![track(a_rx, 1, false), track(b_rx, 1, false)], false);
        set.set_mute(1, true);
        settle(&mut set);
        let mut out = vec![0.0; 256 * 2];
        set.mix_chunk(&mut out);
        let expected = 0.2 * core::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (out[0] - expected).abs() < 1e-6,
            "muted lane contributes nothing"
        );
    }

    #[test]
    fn an_underrun_stalls_without_desyncing_the_lanes() {
        let (mut tx, rx) = RingBuffer::new(1024);
        for _ in 0..100 {
            tx.push(0.5).unwrap();
        }
        let mut set = PlaybackSet::new(vec![track(rx, 1, false)], false);
        let mut out = vec![0.0; 256 * 2];
        let popped = set.mix_chunk(&mut out);
        assert_eq!(popped, 100, "only what the ring held");
        assert_eq!(out[200], 0.0, "the shortfall is silence");
        assert!(!set.shared.finished.load(Ordering::Relaxed));
    }

    #[test]
    fn finished_fires_only_when_every_ring_is_abandoned_and_empty() {
        let (tx, rx) = filled(64, 1, 0.1);
        let mut set = PlaybackSet::new(vec![track(rx, 1, false)], false);
        let mut out = vec![0.0; 64 * 2];
        set.mix_chunk(&mut out);
        assert!(
            !set.shared.finished.load(Ordering::Relaxed),
            "empty but not abandoned = feeder still alive"
        );
        drop(tx);
        set.mix_chunk(&mut out);
        assert!(set.shared.finished.load(Ordering::Relaxed));
    }

    #[test]
    fn a_stale_gate_index_is_ignored() {
        let (_tx, rx) = filled(64, 1, 0.1);
        let mut set = PlaybackSet::new(vec![track(rx, 1, false)], false);
        set.set_solo(7, true);
        set.set_mute(7, true);
        assert!(!set.tracks[0].solo && !set.tracks[0].mute);
    }
}
