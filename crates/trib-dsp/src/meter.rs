use trib_core::{CLIP_DB, db_to_linear};

/// Per-block peak/clip accumulator for one metered point. The audio thread
/// accumulates; `take` resets for the next block.
#[derive(Debug, Clone, Copy)]
pub struct MeterAccum {
    peak: f32,
    clipped: bool,
    clip_linear: f32,
}

impl Default for MeterAccum {
    fn default() -> Self {
        MeterAccum {
            peak: 0.0,
            clipped: false,
            // Precomputed once: the audio thread never does dB math.
            clip_linear: db_to_linear(CLIP_DB),
        }
    }
}

impl MeterAccum {
    #[inline]
    pub fn accumulate(&mut self, sample: f32) {
        let magnitude = sample.abs();
        if magnitude > self.peak {
            self.peak = magnitude;
        }
        self.clipped |= magnitude >= self.clip_linear;
    }

    /// Read and reset: (linear peak, clipped this block).
    pub fn take(&mut self) -> (f32, bool) {
        let out = (self.peak, self.clipped);
        self.peak = 0.0;
        self.clipped = false;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_tracks_the_largest_magnitude_either_sign() {
        let mut m = MeterAccum::default();
        for s in [0.1, -0.7, 0.3] {
            m.accumulate(s);
        }
        let (peak, clipped) = m.take();
        assert_eq!(peak, 0.7);
        assert!(!clipped);
    }

    #[test]
    fn full_scale_counts_as_clip() {
        let mut m = MeterAccum::default();
        m.accumulate(-1.0);
        let (_, clipped) = m.take();
        assert!(clipped);
    }

    #[test]
    fn take_resets_for_the_next_block() {
        let mut m = MeterAccum::default();
        m.accumulate(1.0);
        m.take();
        let (peak, clipped) = m.take();
        assert_eq!(peak, 0.0);
        assert!(!clipped);
    }
}
