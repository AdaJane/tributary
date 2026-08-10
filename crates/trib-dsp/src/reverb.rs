//! The FX-loop reverb: the classic Freeverb topology (8 parallel damped
//! combs into 4 series allpasses), mono in, mono out, 100% wet. Tunings
//! are Jezar's originals at 44.1 kHz, scaled to the running sample rate.

/// Comb delay lengths in samples at the reference 44.1 kHz.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const REFERENCE_RATE: f32 = 44_100.0;
const ALLPASS_FEEDBACK: f32 = 0.5;
/// Freeverb's mapping from the 0..1 room knob to comb feedback.
const ROOM_SCALE: f32 = 0.28;
const ROOM_OFFSET: f32 = 0.7;
/// Damping knob scale (1.0 would fully close the comb lowpass).
const DAMP_SCALE: f32 = 0.4;
/// Output level trim: eight summed combs run hot.
const WET_GAIN: f32 = 0.15;

struct Comb {
    line: Vec<f32>,
    index: usize,
    feedback: f32,
    damp: f32,
    filter_state: f32,
}

impl Comb {
    #[inline]
    fn run(&mut self, input: f32) -> f32 {
        let out = self.line[self.index];
        // One-pole lowpass in the loop: high frequencies die faster,
        // like air absorption.
        self.filter_state = out * (1.0 - self.damp) + self.filter_state * self.damp;
        self.line[self.index] = input + self.filter_state * self.feedback;
        self.index = (self.index + 1) % self.line.len();
        out
    }
}

struct Allpass {
    line: Vec<f32>,
    index: usize,
}

impl Allpass {
    #[inline]
    fn run(&mut self, input: f32) -> f32 {
        let delayed = self.line[self.index];
        let out = delayed - input;
        self.line[self.index] = input + delayed * ALLPASS_FEEDBACK;
        self.index = (self.index + 1) % self.line.len();
        out
    }
}

pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Reverb {
    pub fn new(room_size: f32, damping: f32, sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / REFERENCE_RATE;
        let feedback = room_feedback(room_size);
        let damp = damping.clamp(0.0, 1.0) * DAMP_SCALE;
        Reverb {
            combs: COMB_TUNINGS
                .iter()
                .map(|&len| Comb {
                    line: vec![0.0; ((len as f32 * scale) as usize).max(1)],
                    index: 0,
                    feedback,
                    damp,
                    filter_state: 0.0,
                })
                .collect(),
            allpasses: ALLPASS_TUNINGS
                .iter()
                .map(|&len| Allpass {
                    line: vec![0.0; ((len as f32 * scale) as usize).max(1)],
                    index: 0,
                })
                .collect(),
        }
    }

    pub fn set_room_size(&mut self, room_size: f32) {
        let feedback = room_feedback(room_size);
        for comb in &mut self.combs {
            comb.feedback = feedback;
        }
    }

    pub fn set_damping(&mut self, damping: f32) {
        let damp = damping.clamp(0.0, 1.0) * DAMP_SCALE;
        for comb in &mut self.combs {
            comb.damp = damp;
        }
    }

    /// One sample in, the wet tail out.
    #[inline]
    pub fn run(&mut self, input: f32) -> f32 {
        let mut out = 0.0;
        for comb in &mut self.combs {
            out += comb.run(input);
        }
        for allpass in &mut self.allpasses {
            out = allpass.run(out);
        }
        out * WET_GAIN
    }
}

fn room_feedback(room_size: f32) -> f32 {
    room_size.clamp(0.0, 1.0) * ROOM_SCALE + ROOM_OFFSET
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn tail_energy(reverb: &mut Reverb, from: usize, to: usize) -> f32 {
        let mut energy = 0.0;
        for i in 0..to {
            let out = reverb.run(if i == 0 { 1.0 } else { 0.0 });
            if i >= from {
                energy += out * out;
            }
        }
        energy
    }

    #[test]
    fn an_impulse_grows_a_decaying_tail() {
        let mut reverb = Reverb::new(0.5, 0.5, SR);
        let early = tail_energy(&mut reverb, 0, SR as usize / 4);
        let mut reverb = Reverb::new(0.5, 0.5, SR);
        let late = tail_energy(&mut reverb, SR as usize / 2, SR as usize);
        assert!(early > 0.0, "a tail exists");
        assert!(late < early, "and it decays");
    }

    #[test]
    fn a_bigger_room_rings_longer() {
        let mut small = Reverb::new(0.1, 0.5, SR);
        let mut large = Reverb::new(0.95, 0.5, SR);
        let window = (SR as usize / 2, SR as usize);
        let small_tail = tail_energy(&mut small, window.0, window.1);
        let large_tail = tail_energy(&mut large, window.0, window.1);
        assert!(large_tail > small_tail * 2.0);
    }

    #[test]
    fn the_tail_never_blows_up() {
        let mut reverb = Reverb::new(1.0, 0.0, SR);
        let mut peak = 0.0f32;
        for i in 0..SR as usize * 2 {
            let out = reverb.run(if i % 480 == 0 { 0.5 } else { 0.0 });
            peak = peak.max(out.abs());
        }
        assert!(peak < 2.0, "sustained input stays bounded, peaked {peak}");
    }
}
