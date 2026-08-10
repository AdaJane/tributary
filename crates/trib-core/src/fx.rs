use serde::{Deserialize, Serialize};

use crate::id::{BusId, FxId};

/// Parameters of a built-in effect. The serde tag doubles as the effect kind
/// on the wire, so `FxState` never carries a separate `kind` field to drift.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FxParams {
    Reverb {
        /// 0.0 (booth) .. 1.0 (hall).
        room_size: f32,
        /// 0.0 (bright) .. 1.0 (dark).
        damping: f32,
    },
    Delay {
        time_ms: f32,
        /// 0.0 .. 0.95; capped below unity so the loop always decays.
        feedback: f32,
    },
}

impl FxParams {
    pub const fn kind_str(self) -> &'static str {
        match self {
            FxParams::Reverb { .. } => "reverb",
            FxParams::Delay { .. } => "delay",
        }
    }

    /// Sensible starting points when a unit is created.
    pub const fn default_reverb() -> Self {
        FxParams::Reverb {
            room_size: 0.5,
            damping: 0.5,
        }
    }

    pub const fn default_delay() -> Self {
        FxParams::Delay {
            time_ms: 350.0,
            feedback: 0.35,
        }
    }
}

/// A built-in effect unit: fed by one aux bus, returned into the master at
/// `return_level_db`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FxState {
    pub id: FxId,
    pub name: String,
    pub input: BusId,
    pub params: FxParams,
    pub return_level_db: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_tag_is_the_kind_on_the_wire() {
        let reverb = serde_json::to_value(FxParams::default_reverb()).unwrap();
        assert_eq!(reverb["kind"], "reverb");
        let delay = serde_json::to_value(FxParams::default_delay()).unwrap();
        assert_eq!(delay["kind"], "delay");
        for params in [FxParams::default_reverb(), FxParams::default_delay()] {
            assert_eq!(
                serde_json::to_value(params).unwrap()["kind"],
                params.kind_str()
            );
        }
    }

    #[test]
    fn fx_state_round_trips_through_json() {
        let fx = FxState {
            id: FxId(0),
            name: "Verb".into(),
            input: BusId(1),
            params: FxParams::default_reverb(),
            return_level_db: -6.0,
        };
        let back: FxState = serde_json::from_str(&serde_json::to_string(&fx).unwrap()).unwrap();
        assert_eq!(back, fx);
    }
}
