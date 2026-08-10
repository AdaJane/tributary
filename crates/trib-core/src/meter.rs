use serde::{Deserialize, Serialize};

use crate::id::{BusId, StripId};

/// LED zone boundaries in dBFS, matching classic console calibration:
/// green = signal present with headroom, amber = approaching the top,
/// red = about to run out, clip = digital full scale reached.
pub const LED_AMBER_DB: f32 = -18.0;
pub const LED_RED_DB: f32 = -6.0;
/// Full scale minus an epsilon: a sample at or above this counted as a clip.
pub const CLIP_DB: f32 = -0.1;

/// What a metered point on the console refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeterKey {
    Strip { id: StripId },
    Bus { id: BusId },
    Master,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedZone {
    Green,
    Amber,
    Red,
    Clip,
}

/// Which zone a peak level lights. Pure presentation semantics — the engine
/// reports numbers; this is the one shared reading of them.
pub fn led_zone(peak_db: f32) -> LedZone {
    if peak_db >= CLIP_DB {
        LedZone::Clip
    } else if peak_db >= LED_RED_DB {
        LedZone::Red
    } else if peak_db >= LED_AMBER_DB {
        LedZone::Amber
    } else {
        LedZone::Green
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_map_by_threshold() {
        assert_eq!(led_zone(-40.0), LedZone::Green);
        assert_eq!(led_zone(-18.0), LedZone::Amber);
        assert_eq!(led_zone(-6.0), LedZone::Red);
        assert_eq!(led_zone(-0.1), LedZone::Clip);
        assert_eq!(led_zone(0.0), LedZone::Clip);
    }

    #[test]
    fn meter_key_wire_shape_is_tagged() {
        let strip = serde_json::to_value(MeterKey::Strip { id: StripId(2) }).unwrap();
        assert_eq!(strip, serde_json::json!({ "kind": "strip", "id": 2 }));
        let master = serde_json::to_value(MeterKey::Master).unwrap();
        assert_eq!(master, serde_json::json!({ "kind": "master" }));
    }
}
