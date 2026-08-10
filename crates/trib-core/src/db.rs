//! Decibel math shared by the engine, the API, and persistence.

/// Wire floor for fader/send levels. At or below this the level is −∞ (the
/// signal is fully cut); the UI renders "−∞". Carrying a number instead of a
/// null keeps every consumer's math branch-free.
pub const FADER_MIN_DB: f32 = -90.0;
/// Long-throw faders top out at +10 like the reference console.
pub const FADER_MAX_DB: f32 = 10.0;
/// Input trim range. Digital trim, so wider than a mic pre's printed scale.
pub const GAIN_MIN_DB: f32 = -20.0;
pub const GAIN_MAX_DB: f32 = 60.0;

/// dB → linear amplitude. The floor maps to exactly 0.0 (a true cut, not a
/// -90 dB residue).
pub fn db_to_linear(db: f32) -> f32 {
    if db <= FADER_MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Linear amplitude → dB, floored at `FADER_MIN_DB`. Zero and negative
/// amplitudes are the floor rather than NaN/−∞.
pub fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        return FADER_MIN_DB;
    }
    (20.0 * linear.log10()).max(FADER_MIN_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_is_zero_db() {
        assert_eq!(db_to_linear(0.0), 1.0);
        assert_eq!(linear_to_db(1.0), 0.0);
    }

    #[test]
    fn the_floor_is_a_true_cut() {
        assert_eq!(db_to_linear(FADER_MIN_DB), 0.0);
        assert_eq!(db_to_linear(-120.0), 0.0);
    }

    #[test]
    fn zero_and_negative_amplitudes_floor_instead_of_nan() {
        assert_eq!(linear_to_db(0.0), FADER_MIN_DB);
        assert_eq!(linear_to_db(-1.0), FADER_MIN_DB);
    }

    #[test]
    fn round_trip_within_a_millibel() {
        for db in [-60.0f32, -20.0, -6.0, 0.0, 6.0, 10.0] {
            assert!((linear_to_db(db_to_linear(db)) - db).abs() < 0.001);
        }
    }

    #[test]
    fn six_db_doubles_amplitude_approximately() {
        assert!((db_to_linear(6.0) - 1.9953).abs() < 0.001);
    }
}
