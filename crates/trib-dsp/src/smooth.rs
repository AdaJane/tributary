/// Ramp time for every continuous parameter. Long enough to kill zipper
/// noise on fader grabs, short enough to feel instant under the finger.
pub const SMOOTH_MS: f32 = 10.0;

/// A linearly-smoothed parameter in the linear (amplitude) domain. The
/// control plane sets targets; the audio thread ticks once per sample.
#[derive(Debug, Clone, Copy)]
pub struct SmoothedParam {
    current: f32,
    target: f32,
    /// Per-sample increment magnitude for the active ramp.
    step: f32,
    /// Samples per ramp, fixed by the sample rate at construction.
    ramp_samples: f32,
}

impl SmoothedParam {
    pub fn new(initial: f32, sample_rate: u32) -> Self {
        SmoothedParam {
            current: initial,
            target: initial,
            step: 0.0,
            ramp_samples: (SMOOTH_MS / 1000.0) * sample_rate as f32,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
        self.step = (target - self.current) / self.ramp_samples;
    }

    /// Advance one sample and return the value to use for it.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.current != self.target {
            let next = self.current + self.step;
            // Ramp done when the step crosses (or lands on) the target.
            self.current = if (self.target - next).is_sign_positive()
                == (self.target - self.current).is_sign_positive()
            {
                next
            } else {
                self.target
            };
        }
        self.current
    }

    pub fn value(&self) -> f32 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    #[test]
    fn a_new_param_holds_its_value() {
        let mut p = SmoothedParam::new(0.5, SR);
        for _ in 0..64 {
            assert_eq!(p.tick(), 0.5);
        }
    }

    #[test]
    fn reaches_the_target_within_the_ramp_and_stays() {
        let mut p = SmoothedParam::new(0.0, SR);
        p.set_target(1.0);
        let ramp = ((SMOOTH_MS / 1000.0) * SR as f32) as usize;
        for _ in 0..ramp + 2 {
            p.tick();
        }
        assert_eq!(p.value(), 1.0);
        assert_eq!(p.tick(), 1.0, "no oscillation after arrival");
    }

    #[test]
    fn ramps_monotonically_without_overshoot() {
        let mut p = SmoothedParam::new(1.0, SR);
        p.set_target(0.25);
        let mut last = p.value();
        for _ in 0..2_000 {
            let v = p.tick();
            assert!(v <= last, "downward ramp must not bounce");
            assert!(v >= 0.25, "must not overshoot the target");
            last = v;
        }
        assert_eq!(last, 0.25);
    }

    #[test]
    fn retargeting_mid_ramp_turns_around_cleanly() {
        let mut p = SmoothedParam::new(0.0, SR);
        p.set_target(1.0);
        for _ in 0..100 {
            p.tick();
        }
        let mid = p.value();
        p.set_target(0.0);
        for _ in 0..1_000 {
            p.tick();
        }
        assert_eq!(p.value(), 0.0);
        assert!(mid > 0.0, "the first ramp had begun");
    }
}
