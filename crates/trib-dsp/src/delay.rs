//! The FX-loop delay: a preallocated line with smoothed read offset and
//! bounded feedback. 100% wet — dry/wet balance is the console's job
//! (send level vs return level), like an outboard unit.

use crate::smooth::SmoothedParam;

/// Longest supported delay. The line is allocated once at this size;
/// `time_ms` is a read offset, so changing it never allocates.
pub const MAX_DELAY_MS: f32 = 2_000.0;
/// Feedback ceiling: the loop must always decay.
pub const MAX_FEEDBACK: f32 = 0.95;

pub struct Delay {
    line: Vec<f32>,
    write: usize,
    /// Read offset in samples, smoothed — a moving time knob glides
    /// (tape-style) instead of clicking.
    offset: SmoothedParam,
    feedback: SmoothedParam,
    sample_rate: u32,
}

impl Delay {
    pub fn new(time_ms: f32, feedback: f32, sample_rate: u32) -> Self {
        let capacity = (MAX_DELAY_MS / 1000.0 * sample_rate as f32) as usize + 1;
        Delay {
            line: vec![0.0; capacity],
            write: 0,
            offset: SmoothedParam::new(Self::offset_samples(time_ms, sample_rate), sample_rate),
            feedback: SmoothedParam::new(feedback.clamp(0.0, MAX_FEEDBACK), sample_rate),
            sample_rate,
        }
    }

    fn offset_samples(time_ms: f32, sample_rate: u32) -> f32 {
        (time_ms.clamp(1.0, MAX_DELAY_MS) / 1000.0 * sample_rate as f32).max(1.0)
    }

    pub fn set_time_ms(&mut self, time_ms: f32) {
        self.offset
            .set_target(Self::offset_samples(time_ms, self.sample_rate));
    }

    pub fn set_feedback(&mut self, feedback: f32) {
        self.feedback.set_target(feedback.clamp(0.0, MAX_FEEDBACK));
    }

    /// One sample in, the delayed (wet) sample out.
    #[inline]
    pub fn run(&mut self, input: f32) -> f32 {
        let offset = self.offset.tick() as usize;
        let len = self.line.len();
        let read = (self.write + len - offset.min(len - 1)) % len;
        let wet = self.line[read];
        self.line[self.write] = input + wet * self.feedback.tick();
        self.write = (self.write + 1) % len;
        wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    #[test]
    fn an_impulse_returns_after_the_delay_time() {
        let mut delay = Delay::new(10.0, 0.0, SR);
        let delay_samples = (0.010 * SR as f32) as usize;
        let mut first_echo = None;
        for i in 0..delay_samples * 2 {
            let out = delay.run(if i == 0 { 1.0 } else { 0.0 });
            if out.abs() > 0.5 && first_echo.is_none() {
                first_echo = Some(i);
            }
        }
        assert_eq!(first_echo, Some(delay_samples));
    }

    #[test]
    fn feedback_produces_decaying_repeats() {
        let mut delay = Delay::new(5.0, 0.5, SR);
        let period = (0.005 * SR as f32) as usize;
        let mut peaks = Vec::new();
        for i in 0..period * 4 {
            let out = delay.run(if i == 0 { 1.0 } else { 0.0 });
            if out.abs() > 0.05 {
                peaks.push(out.abs());
            }
        }
        assert!(peaks.len() >= 3, "several repeats: {peaks:?}");
        assert!(peaks[1] < peaks[0], "each repeat is quieter");
    }

    #[test]
    fn feedback_is_capped_below_unity() {
        let mut delay = Delay::new(5.0, 2.0, SR);
        // A runaway loop would blow past 1.0 within a few periods.
        let mut peak = 0.0f32;
        for i in 0..SR as usize {
            let out = delay.run(if i == 0 { 1.0 } else { 0.0 });
            peak = peak.max(out.abs());
        }
        assert!(peak <= 1.0, "loop must decay, peaked at {peak}");
    }
}
