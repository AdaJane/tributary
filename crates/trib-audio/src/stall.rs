//! Output-stall detection. A routing change (e.g. a Bluetooth sink
//! renegotiating) can silently stop output callbacks without an error —
//! costing monitor audio only (the engine clocks itself on its own
//! thread). The stream thread feeds beats into this pure core, logs the
//! transitions, and uses the stalled state to gate rebuilds. A stream
//! that never beat — none was built at boot — reads as stalled by design.

use std::time::{Duration, Instant};

/// No callback for this long = stalled. Normal scheduling jitter is
/// milliseconds; Bluetooth renegotiation is seconds.
pub const STALL_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub enum Transition {
    Stalled,
    /// Carries how long the outage lasted.
    Resumed(Duration),
}

pub struct StallWatch {
    last_beat: u64,
    last_advance: Instant,
    stalled: bool,
}

impl StallWatch {
    pub fn new(now: Instant) -> StallWatch {
        StallWatch {
            last_beat: 0,
            last_advance: now,
            stalled: false,
        }
    }

    /// Observe the callback's beat counter. Returns a transition to log,
    /// at most once per stall in each direction.
    pub fn observe(&mut self, beat: u64, now: Instant) -> Option<Transition> {
        if beat != self.last_beat {
            self.last_beat = beat;
            let outage = now.duration_since(self.last_advance);
            self.last_advance = now;
            if self.stalled {
                self.stalled = false;
                return Some(Transition::Resumed(outage));
            }
            return None;
        }
        if !self.stalled && now.duration_since(self.last_advance) >= STALL_AFTER {
            self.stalled = true;
            return Some(Transition::Stalled);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(start: Instant, secs: f32) -> Instant {
        start + Duration::from_secs_f32(secs)
    }

    #[test]
    fn a_beating_stream_never_reports() {
        let start = Instant::now();
        let mut watch = StallWatch::new(start);
        for i in 1..50 {
            assert_eq!(watch.observe(i, t(start, i as f32 * 0.1)), None);
        }
    }

    #[test]
    fn silence_past_the_threshold_reports_stalled_once() {
        let start = Instant::now();
        let mut watch = StallWatch::new(start);
        assert_eq!(watch.observe(1, t(start, 0.1)), None);
        assert_eq!(watch.observe(1, t(start, 1.0)), None, "under threshold");
        assert_eq!(watch.observe(1, t(start, 2.2)), Some(Transition::Stalled));
        assert_eq!(watch.observe(1, t(start, 3.0)), None, "no repeat");
    }

    #[test]
    fn a_stream_that_never_beats_reports_stalled() {
        // Pins never-built ≡ wedged: `new` starts at beat 0, so a stream
        // that was never created stalls without ever advancing.
        let start = Instant::now();
        let mut watch = StallWatch::new(start);
        assert_eq!(watch.observe(0, t(start, 0.1)), None, "under threshold");
        assert_eq!(watch.observe(0, t(start, 2.2)), Some(Transition::Stalled));
        assert_eq!(watch.observe(0, t(start, 60.0)), None, "no repeat");
    }

    #[test]
    fn the_first_beat_ever_after_a_boot_stall_resumes_with_the_full_outage() {
        let start = Instant::now();
        let mut watch = StallWatch::new(start);
        assert_eq!(watch.observe(0, t(start, 2.5)), Some(Transition::Stalled));
        let resumed = watch.observe(1, t(start, 30.0));
        let Some(Transition::Resumed(outage)) = resumed else {
            panic!("expected resume, got {resumed:?}");
        };
        let secs = outage.as_secs_f32();
        assert!(
            (29.9..=30.1).contains(&secs),
            "outage spans back to thread start, got {secs}"
        );
    }

    #[test]
    fn the_first_beat_after_a_stall_reports_the_outage_length() {
        let start = Instant::now();
        let mut watch = StallWatch::new(start);
        assert_eq!(watch.observe(1, t(start, 0.1)), None);
        assert_eq!(watch.observe(1, t(start, 2.5)), Some(Transition::Stalled));
        let resumed = watch.observe(2, t(start, 5.1));
        let Some(Transition::Resumed(outage)) = resumed else {
            panic!("expected resume, got {resumed:?}");
        };
        let secs = outage.as_secs_f32();
        assert!(
            (4.9..=5.1).contains(&secs),
            "outage spans back to the last beat, got {secs}"
        );
        assert_eq!(watch.observe(3, t(start, 5.2)), None, "healthy again");
    }
}
