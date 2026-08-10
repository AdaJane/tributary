use serde::{Deserialize, Serialize};

use crate::bus::BusState;
use crate::fx::FxState;
use crate::id::{BusId, StripId};
use crate::strip::StripState;

/// The master output section: one stereo fader and its own record arm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MasterState {
    pub fader_db: f32,
    pub record_arm: bool,
}

impl Default for MasterState {
    fn default() -> Self {
        MasterState {
            fader_db: 0.0,
            record_arm: false,
        }
    }
}

/// The whole console document: the single authoritative mix state the daemon
/// owns, the UI mirrors, and the project manifest persists.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MixerState {
    pub strips: Vec<StripState>,
    pub buses: Vec<BusState>,
    pub fx: Vec<FxState>,
    pub master: MasterState,
}

impl MixerState {
    pub fn strip(&self, id: StripId) -> Option<&StripState> {
        self.strips.iter().find(|s| s.id == id)
    }

    pub fn bus(&self, id: BusId) -> Option<&BusState> {
        self.buses.iter().find(|b| b.id == id)
    }

    /// The next unused strip id. Ids are never reused within a project so a
    /// removed strip's takes stay attributable.
    pub fn next_strip_id(&self) -> StripId {
        StripId(self.strips.iter().map(|s| s.id.0 + 1).max().unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::BusKind;
    use crate::fx::FxParams;
    use crate::id::FxId;

    fn fixture() -> MixerState {
        MixerState {
            strips: vec![
                StripState::new(StripId(0), "Vocal".into()),
                StripState::new(StripId(1), "Guitar".into()),
            ],
            buses: vec![BusState::new(BusId(0), BusKind::Aux, "FX 1".into())],
            fx: vec![FxState {
                id: FxId(0),
                name: "Verb".into(),
                input: BusId(0),
                params: FxParams::default_reverb(),
                return_level_db: -6.0,
            }],
            master: MasterState::default(),
        }
    }

    #[test]
    fn a_full_document_round_trips_through_json() {
        let state = fixture();
        let back: MixerState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn lookups_find_by_id_not_position() {
        let state = fixture();
        assert_eq!(state.strip(StripId(1)).unwrap().name, "Guitar");
        assert!(state.strip(StripId(9)).is_none());
        assert_eq!(state.bus(BusId(0)).unwrap().name, "FX 1");
    }

    #[test]
    fn next_strip_id_never_reuses_an_id() {
        let mut state = fixture();
        assert_eq!(state.next_strip_id(), StripId(2));
        state.strips.remove(0);
        assert_eq!(state.next_strip_id(), StripId(2), "gap is not recycled");
        state.strips.clear();
        assert_eq!(state.next_strip_id(), StripId(0));
    }
}
