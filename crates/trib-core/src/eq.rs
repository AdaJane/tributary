use serde::{Deserialize, Serialize};

use crate::ParseEnumError;

/// EQ gain knobs sweep ±15 dB, matching the reference console's printed scale.
pub const EQ_GAIN_RANGE_DB: f32 = 15.0;
/// Swept-mid frequency range. Wider than the Folio's 240 Hz–6 kHz because a
/// digital sweep costs nothing and kick/air work wants the extra reach.
pub const MID_FREQ_MIN_HZ: f32 = 100.0;
pub const MID_FREQ_MAX_HZ: f32 = 8_000.0;
/// Absolute band-frequency bounds the reducer accepts (shelves included).
pub const EQ_FREQ_MIN_HZ: f32 = 20.0;
pub const EQ_FREQ_MAX_HZ: f32 = 20_000.0;
/// Q bounds: below 0.1 the filter is a wire, above 10 it's a whistle.
pub const EQ_Q_MIN: f32 = 0.1;
pub const EQ_Q_MAX: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EqBandKind {
    LowShelf,
    Peak,
    HighShelf,
}

impl EqBandKind {
    pub const ALL: [EqBandKind; 3] = [
        EqBandKind::LowShelf,
        EqBandKind::Peak,
        EqBandKind::HighShelf,
    ];

    /// One name serves the JSON wire, the TOML manifest, and log lines; a test
    /// asserts serde agrees.
    pub const fn as_str(self) -> &'static str {
        match self {
            EqBandKind::LowShelf => "low_shelf",
            EqBandKind::Peak => "peak",
            EqBandKind::HighShelf => "high_shelf",
        }
    }
}

impl std::str::FromStr for EqBandKind {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EqBandKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| ParseEnumError {
                kind: "eq band",
                value: s.to_owned(),
            })
    }
}

/// One EQ band of a strip. `kind` is fixed per slot (low/mid/high); freq is
/// only user-swept on the mid band, but carrying it uniformly keeps the DSP
/// coefficient path shape-free.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EqBand {
    pub kind: EqBandKind,
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

/// The Folio-style 3-band strip EQ: LF shelf, swept mid peak, HF shelf.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelEq {
    pub enabled: bool,
    pub low: EqBand,
    pub mid: EqBand,
    pub high: EqBand,
}

impl Default for ChannelEq {
    /// Flat response at classic console centers, and OFF: a fresh channel
    /// adds no coloration until the operator reaches for the EQ.
    fn default() -> Self {
        let band = |kind, freq_hz| EqBand {
            kind,
            freq_hz,
            gain_db: 0.0,
            q: 0.71,
        };
        ChannelEq {
            enabled: false,
            low: band(EqBandKind::LowShelf, 80.0),
            mid: band(EqBandKind::Peak, 800.0),
            high: band(EqBandKind::HighShelf, 12_000.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_agrees_with_serde_for_every_kind() {
        for kind in EqBandKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(json.trim_matches('"').parse::<EqBandKind>().unwrap(), kind);
        }
    }

    #[test]
    fn unknown_band_name_is_a_parse_error() {
        assert!("bell".parse::<EqBandKind>().is_err());
    }

    #[test]
    fn default_eq_is_flat_and_off() {
        let eq = ChannelEq::default();
        assert!(!eq.enabled, "a fresh channel colors nothing");
        for band in [eq.low, eq.mid, eq.high] {
            assert_eq!(band.gain_db, 0.0);
        }
        assert_eq!(eq.mid.kind, EqBandKind::Peak);
    }

    #[test]
    fn eq_round_trips_through_json() {
        let eq = ChannelEq::default();
        let back: ChannelEq = serde_json::from_str(&serde_json::to_string(&eq).unwrap()).unwrap();
        assert_eq!(back, eq);
    }
}
