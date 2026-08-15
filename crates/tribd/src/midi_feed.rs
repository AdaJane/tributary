//! Playing a take's MIDI back out to hardware.
//!
//! One thread per route, reading a sidecar that was loaded off the runtime
//! before playback started, and emitting against the PLAYHEAD — never the
//! wall clock. `PlaybackShared::position` is advanced by real frames popped
//! in the audio callback, so it *is* the audio clock; timing against
//! `Instant::now()` instead would drift against the take over a long song,
//! the same failure `midi.rs` calls out for accumulating tick deltas.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use trib_engine::PlaybackShared;
use trib_project::SidecarEvent;

use crate::midi_echo::Outgoing;
use crate::midi_out::OutPorts;

/// Top-up cadence.
///
/// Ten times faster than the take feeder's 50 ms, and for the opposite
/// reason: an audio feeder buys four seconds of ring, so its interval is
/// invisible. A MIDI feeder has no ring — its interval IS its jitter, and
/// 50 ms of jitter on a note is audible where 5 ms is not.
const FEED_INTERVAL: Duration = Duration::from_millis(5);

/// What one feeder plays.
pub struct MidiPlayback {
    /// Read off the runtime BEFORE playback starts: opening a file is not
    /// something to do between two notes.
    pub events: Vec<SidecarEvent>,
    pub port: String,
    /// `None` = pass the file's own channel through.
    pub channel: Option<u8>,
    pub shared: Arc<PlaybackShared>,
    /// Where in the take playback began, so a seek starts in the right
    /// place rather than at the top.
    pub start_frame: u64,
    /// Inclusive start, exclusive end, in take frames.
    pub loop_region: Option<(u64, u64)>,
}

/// Stops the feeder when dropped, and silences what it started.
pub struct MidiFeederHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MidiFeederHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Where the playhead is on the take's own timeline, folding loop passes.
///
/// The same mapping the transport's position ticker uses; a feeder with
/// its own copy would let the notes and the playhead readout disagree.
pub fn timeline_position(start_frame: u64, loop_region: Option<(u64, u64)>, played: u64) -> u64 {
    let pos = start_frame + played;
    match loop_region {
        Some((start, end)) if end > start && pos >= end => start + (pos - end) % (end - start),
        _ => pos,
    }
}

/// Every event strictly after `from`, up to and including `to`.
///
/// Half-open at the bottom so an event exactly on a window boundary is
/// emitted once and only once — the same note re-sent on every tick is a
/// machine-gun, and on every loop pass is a stuck chord.
pub fn due(events: &[SidecarEvent], from: u64, to: u64) -> &[SidecarEvent] {
    let start = events.partition_point(|e| e.sample <= from);
    let end = events.partition_point(|e| e.sample <= to);
    &events[start..end.max(start)]
}

/// Every event from `from` up to and including `to`, INCLUSIVE at both
/// ends.
///
/// For the first window of a pass, where the lower bound is a position
/// nothing has played yet rather than a position already covered. Without
/// it a note on the very first frame — the downbeat, or the first note
/// after a loop wraps — is silently swallowed.
pub fn due_from(events: &[SidecarEvent], from: u64, to: u64) -> &[SidecarEvent] {
    let start = events.partition_point(|e| e.sample < from);
    let end = events.partition_point(|e| e.sample <= to);
    &events[start..end.max(start)]
}

pub fn spawn(play: MidiPlayback, ports: Arc<OutPorts>) -> MidiFeederHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("trib-midi-feed".into())
        .spawn(move || run(play, ports, flag))
        .ok();
    MidiFeederHandle { stop, thread }
}

fn run(play: MidiPlayback, ports: Arc<OutPorts>, stop: Arc<AtomicBool>) {
    // `None` = nothing has been emitted for this pass yet, so the next
    // window is inclusive at its lower bound. A plain `last = start_frame`
    // would swallow a note sitting exactly on the downbeat.
    let mut last: Option<u64> = None;
    let mut floor = play.start_frame;
    while !stop.load(Ordering::Relaxed) {
        let played = play.shared.position.load(Ordering::Relaxed);
        let now = timeline_position(play.start_frame, play.loop_region, played);

        if last.is_some_and(|last| now < last) {
            // A loop wrapped. Everything the region started is released
            // before the region replays it, or every pass would stack
            // another held note on the synth — and nothing in the room can
            // reach an external one to stop it.
            ports.panic_port(&play.port);
            floor = play
                .loop_region
                .map_or(play.start_frame, |(start, _)| start);
            last = None;
        }

        let window = match last {
            None => due_from(&play.events, floor, now),
            Some(from) => due(&play.events, from, now),
        };
        for event in window {
            ports.send(&Outgoing {
                port: play.port.clone(),
                channel: play.channel.unwrap_or(event.status & 0x0F),
                status: event.status,
                data1: event.data1,
                data2: event.data2,
            });
        }
        last = Some(now);

        if play.shared.finished.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(FEED_INTERVAL);
    }
    // However this ends — the take ran out, the transport stopped, the
    // route was removed — nothing may be left holding a note on hardware
    // nobody in the room can reach.
    ports.panic_port(&play.port);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sample: u64) -> SidecarEvent {
        SidecarEvent {
            sample,
            status: 0x90,
            data1: 60,
            data2: 100,
        }
    }

    #[test]
    fn an_event_on_a_window_boundary_is_emitted_once_and_only_once() {
        // The bug this prevents: overlapping windows re-send the note on
        // every tick, so a held chord becomes a machine-gun.
        let events = [event(0), event(100), event(200)];
        assert_eq!(due(&events, 0, 100).len(), 1);
        assert_eq!(due(&events, 100, 200).len(), 1);
        assert_eq!(due(&events, 100, 100).len(), 0, "an empty window is empty");
    }

    #[test]
    fn the_first_window_of_a_pass_includes_a_note_on_the_downbeat() {
        // The bug this exists for: an exclusive lower bound swallows an
        // event sitting exactly on the frame playback started from — the
        // take's first note, and the first note of every loop pass.
        let events = [event(0), event(100)];
        assert_eq!(due_from(&events, 0, 0).len(), 1);
        assert_eq!(due(&events, 0, 0).len(), 0, "the exclusive form does not");
        assert_eq!(due_from(&events, 100, 200).len(), 1, "and at a loop start");
    }

    #[test]
    fn the_playhead_folds_loop_passes_onto_the_takes_own_timeline() {
        let region = Some((100, 200));
        assert_eq!(timeline_position(100, region, 0), 100);
        assert_eq!(timeline_position(100, region, 99), 199);
        assert_eq!(timeline_position(100, region, 100), 100, "wrapped");
        assert_eq!(timeline_position(100, region, 150), 150);
    }

    #[test]
    fn a_zero_width_loop_region_does_not_divide_by_zero() {
        assert_eq!(timeline_position(0, Some((100, 100)), 500), 500);
    }

    #[test]
    fn without_a_loop_the_position_is_simply_where_it_started_plus_progress() {
        assert_eq!(timeline_position(48_000, None, 24_000), 72_000);
    }
}
