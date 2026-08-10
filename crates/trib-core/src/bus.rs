use serde::{Deserialize, Serialize};

use crate::ParseEnumError;
use crate::id::BusId;

/// What a bus feeds. Groups mix into the master; aux buses feed an FX unit.
/// The split is the v1 cycle-prevention stage rule: strips → groups → aux →
/// fx → master, never backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum BusKind {
    Group,
    Aux,
}

impl BusKind {
    pub const ALL: [BusKind; 2] = [BusKind::Group, BusKind::Aux];

    pub const fn as_str(self) -> &'static str {
        match self {
            BusKind::Group => "group",
            BusKind::Aux => "aux",
        }
    }
}

impl std::str::FromStr for BusKind {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BusKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| ParseEnumError {
                kind: "bus kind",
                value: s.to_owned(),
            })
    }
}

/// A group or aux bus strip. Renders like a channel strip without gain/arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BusState {
    pub id: BusId,
    pub kind: BusKind,
    pub name: String,
    pub fader_db: f32,
    /// Constant-power pan of a group into the master. Ignored for aux buses
    /// (their output feeds an FX unit input, which is mono-summed).
    pub pan: f32,
    pub mute: bool,
    pub pfl: bool,
}

impl BusState {
    /// A fresh bus: unity fader. Buses carry already-checked signals, so the
    /// safe-start rule that puts strip faders down does not apply.
    pub fn new(id: BusId, kind: BusKind, name: String) -> Self {
        BusState {
            id,
            kind,
            name,
            fader_db: 0.0,
            pan: 0.0,
            mute: false,
            pfl: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_agrees_with_serde_for_every_kind() {
        for kind in BusKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(json.trim_matches('"').parse::<BusKind>().unwrap(), kind);
        }
    }

    #[test]
    fn a_fresh_bus_starts_at_unity() {
        let bus = BusState::new(BusId(0), BusKind::Group, "Drums".into());
        assert_eq!(bus.fader_db, 0.0);
        assert!(!bus.mute && !bus.pfl);
    }
}
