//! EQ filter math. Coefficients are computed CONTROL-SIDE from validated
//! band parameters; the audio thread only installs them.

use biquad::{Coefficients, Type};
use trib_core::{EqBand, EqBandKind};

/// The per-band filter state that runs on the audio thread.
pub type BandFilter = biquad::DirectForm2Transposed<f32>;

/// RBJ coefficients for one band. The reducer's range checks are the
/// precondition (finite gain, 20 Hz–20 kHz below Nyquist at ≥44.1 kHz,
/// sane Q); violating them is a bug upstream, so this panics rather than
/// limping.
pub fn band_coefficients(band: &EqBand, sample_rate: u32) -> Coefficients<f32> {
    let kind = match band.kind {
        EqBandKind::LowShelf => Type::LowShelf(band.gain_db),
        EqBandKind::Peak => Type::PeakingEQ(band.gain_db),
        EqBandKind::HighShelf => Type::HighShelf(band.gain_db),
    };
    // NOT `from_params`: biquad 0.5 normalizes it as f0/(2·fs) but then uses
    // ω = π·norm, landing the filter at f0/4 (verified by the magnitude
    // tests below). Feeding `from_normalized_params` 2·f0/fs makes
    // ω = 2π·f0/fs — the RBJ cookbook's ω0 — and norm < 1 becomes exactly
    // the Nyquist bound.
    let normalized = 2.0 * band.freq_hz / sample_rate as f32;
    Coefficients::<f32>::from_normalized_params(kind, normalized, band.q)
        .expect("validated band parameters produce coefficients")
}

#[cfg(test)]
mod tests {
    use biquad::Biquad;

    use super::*;

    const SR: u32 = 48_000;

    /// Steady-state amplitude ratio of a sine at `freq` through the filter.
    fn gain_at(band: &EqBand, freq: f32) -> f32 {
        let mut filter = BandFilter::new(band_coefficients(band, SR));
        let samples = SR as usize; // one second: plenty of settling
        let mut peak_in = 0.0f32;
        let mut peak_out = 0.0f32;
        for i in 0..samples {
            let x = (i as f32 / SR as f32 * freq * std::f32::consts::TAU).sin();
            let y = filter.run(x);
            // Measure only after settling.
            if i > samples / 2 {
                peak_in = peak_in.max(x.abs());
                peak_out = peak_out.max(y.abs());
            }
        }
        peak_out / peak_in
    }

    fn band(kind: EqBandKind, freq_hz: f32, gain_db: f32) -> EqBand {
        EqBand {
            kind,
            freq_hz,
            gain_db,
            q: 0.71,
        }
    }

    #[test]
    fn a_flat_band_is_a_wire() {
        let flat = band(EqBandKind::Peak, 1_000.0, 0.0);
        let g = gain_at(&flat, 1_000.0);
        assert!((g - 1.0).abs() < 0.01, "flat gain was {g}");
    }

    #[test]
    fn a_six_db_peak_boosts_its_center_by_six_db() {
        let peak = band(EqBandKind::Peak, 1_000.0, 6.0);
        let g = 20.0 * gain_at(&peak, 1_000.0).log10();
        assert!((g - 6.0).abs() < 0.3, "center gain was {g} dB");
    }

    #[test]
    fn a_peak_leaves_distant_frequencies_alone() {
        let peak = band(EqBandKind::Peak, 1_000.0, 12.0);
        let g = 20.0 * gain_at(&peak, 60.0).log10();
        assert!(g.abs() < 1.0, "gain four octaves down was {g} dB");
    }

    #[test]
    fn a_low_shelf_boosts_below_and_spares_above() {
        let shelf = band(EqBandKind::LowShelf, 200.0, 6.0);
        let low = 20.0 * gain_at(&shelf, 50.0).log10();
        let high = 20.0 * gain_at(&shelf, 5_000.0).log10();
        assert!((low - 6.0).abs() < 0.5, "shelf floor was {low} dB");
        assert!(high.abs() < 0.5, "highs moved by {high} dB");
    }

    #[test]
    fn a_high_shelf_cut_attenuates_the_top() {
        // Two octaves above the corner: the shelf has fully plateaued.
        let shelf = band(EqBandKind::HighShelf, 4_000.0, -9.0);
        let top = 20.0 * gain_at(&shelf, 16_000.0).log10();
        assert!((top + 9.0).abs() < 1.0, "shelf ceiling was {top} dB");
    }
}
