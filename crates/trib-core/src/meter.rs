use serde::{Deserialize, Serialize};

use crate::id::{BusId, StripId};

/// Full scale minus an epsilon: a sample at or above this counted as a clip.
///
/// The only calibration the daemon owns. LED zone boundaries deliberately
/// do NOT live here — they are presentation, they differ between the
/// channel and master meters, and the design system (`LedMeter` in
/// design-system/tributary/MASTER.md) is their single source of truth.
/// A second, unused copy lived here and disagreed with it for long enough
/// to be quoted as fact in two doc comments.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_key_wire_shape_is_tagged() {
        let strip = serde_json::to_value(MeterKey::Strip { id: StripId(2) }).unwrap();
        assert_eq!(strip, serde_json::json!({ "kind": "strip", "id": 2 }));
        let master = serde_json::to_value(MeterKey::Master).unwrap();
        assert_eq!(master, serde_json::json!({ "kind": "master" }));
    }
}
