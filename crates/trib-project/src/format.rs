//! Recording formats: the legal container × bit-depth matrix, plus the
//! f32 → integer quantizer shared by every non-float encoder.

use serde::{Deserialize, Serialize};

/// What the take writer puts on disk. Bit depth folds into the variant so
/// illegal combinations (float FLAC) are unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecordFormat {
    Wav16,
    Wav24,
    #[default]
    Wav32Float,
    Flac16,
    Flac24,
}

impl RecordFormat {
    pub const fn extension(self) -> &'static str {
        match self {
            RecordFormat::Wav16 | RecordFormat::Wav24 | RecordFormat::Wav32Float => "wav",
            RecordFormat::Flac16 | RecordFormat::Flac24 => "flac",
        }
    }

    pub const fn bits_per_sample(self) -> u16 {
        match self {
            RecordFormat::Wav16 | RecordFormat::Flac16 => 16,
            RecordFormat::Wav24 | RecordFormat::Flac24 => 24,
            RecordFormat::Wav32Float => 32,
        }
    }

    pub const fn is_float(self) -> bool {
        matches!(self, RecordFormat::Wav32Float)
    }
}

/// Clamp to ±1.0 and scale to a signed integer of `bits` — symmetric
/// (±full-scale maps to ±(2^(bits-1) − 1)), matching the peaks scaler.
pub(crate) fn quantize(sample: f32, bits: u16) -> i32 {
    let max = ((1_i64 << (bits - 1)) - 1) as f32;
    (sample.clamp(-1.0, 1.0) * max).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [RecordFormat; 5] = [
        RecordFormat::Wav16,
        RecordFormat::Wav24,
        RecordFormat::Wav32Float,
        RecordFormat::Flac16,
        RecordFormat::Flac24,
    ];

    #[test]
    fn serde_names_round_trip_every_variant() {
        for (format, name) in ALL
            .iter()
            .zip(["wav16", "wav24", "wav32_float", "flac16", "flac24"])
        {
            let value = toml::Value::try_from(format).unwrap();
            assert_eq!(value, toml::Value::String(name.into()));
            let back: RecordFormat = value.try_into().unwrap();
            assert_eq!(back, *format);
        }
    }

    #[test]
    fn an_unknown_format_name_is_refused() {
        assert!(
            toml::Value::String("mp3".into())
                .try_into::<RecordFormat>()
                .is_err()
        );
    }

    #[test]
    fn the_default_is_the_legacy_format() {
        assert_eq!(RecordFormat::default(), RecordFormat::Wav32Float);
    }

    #[test]
    fn extension_bits_and_floatness_cover_the_matrix() {
        let table = [
            (RecordFormat::Wav16, "wav", 16, false),
            (RecordFormat::Wav24, "wav", 24, false),
            (RecordFormat::Wav32Float, "wav", 32, true),
            (RecordFormat::Flac16, "flac", 16, false),
            (RecordFormat::Flac24, "flac", 24, false),
        ];
        for (format, ext, bits, float) in table {
            assert_eq!(format.extension(), ext);
            assert_eq!(format.bits_per_sample(), bits);
            assert_eq!(format.is_float(), float);
        }
    }

    #[test]
    fn quantize_clamps_at_full_scale() {
        assert_eq!(quantize(1.0, 16), 32_767);
        assert_eq!(quantize(-1.0, 16), -32_767);
        assert_eq!(quantize(2.0, 16), 32_767, "over-range clamps");
        assert_eq!(quantize(-2.0, 24), -8_388_607);
        assert_eq!(quantize(1.0, 24), 8_388_607);
    }

    #[test]
    fn quantize_is_symmetric_and_rounds_at_the_lsb() {
        assert_eq!(quantize(0.0, 16), 0);
        for v in [0.1_f32, 0.25, 0.5, 0.999] {
            assert_eq!(quantize(-v, 24), -quantize(v, 24));
        }
        // Half an LSB rounds away from zero, not truncates.
        let half_lsb = 0.5 / 32_767.0;
        assert_eq!(quantize(half_lsb, 16), 1);
        assert_eq!(quantize(half_lsb * 0.9, 16), 0);
    }
}
