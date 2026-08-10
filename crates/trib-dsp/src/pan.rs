/// Constant-power pan law: −3 dB at center, so a source keeps its perceived
/// loudness as it sweeps.
#[inline]
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * core::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_is_minus_three_db_both_sides() {
        let (l, r) = pan_gains(0.0);
        let minus_3_db = core::f32::consts::FRAC_1_SQRT_2;
        assert!((l - minus_3_db).abs() < 1e-6);
        assert!((r - minus_3_db).abs() < 1e-6);
    }

    #[test]
    fn hard_left_and_right_are_exclusive() {
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6);
        assert!(r.abs() < 1e-6);
        let (l, r) = pan_gains(1.0);
        assert!(l.abs() < 1e-6);
        assert!((r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn power_is_constant_across_the_sweep() {
        for i in 0..=20 {
            let (l, r) = pan_gains(-1.0 + i as f32 / 10.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-5);
        }
    }
}
