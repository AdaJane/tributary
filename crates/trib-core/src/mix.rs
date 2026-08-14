use serde::{Deserialize, Serialize};

use crate::bus::BusState;
use crate::fx::FxState;
use crate::id::{BusId, InstrumentId, StripId};
use crate::instrument::InstrumentState;
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
    /// Declared after the other arrays and before `master` on purpose:
    /// `toml` serializes in declaration order and refuses a value after a
    /// table, so a scalar section between the arrays-of-tables would make
    /// every manifest unwritable.
    #[serde(default)]
    pub instruments: Vec<InstrumentState>,
    pub master: MasterState,
}

impl MixerState {
    pub fn strip(&self, id: StripId) -> Option<&StripState> {
        self.strips.iter().find(|s| s.id == id)
    }

    pub fn bus(&self, id: BusId) -> Option<&BusState> {
        self.buses.iter().find(|b| b.id == id)
    }

    pub fn instrument(&self, id: InstrumentId) -> Option<&InstrumentState> {
        self.instruments.iter().find(|i| i.id == id)
    }

    /// The next unused strip id. Ids are never reused within a project so a
    /// removed strip's takes stay attributable.
    pub fn next_strip_id(&self) -> StripId {
        StripId(self.strips.iter().map(|s| s.id.0 + 1).max().unwrap_or(0))
    }

    /// The next unused instrument id. Never reused either: the id is the
    /// printed slot, the patch identity a strip holds, and the seed for the
    /// link tape's colour.
    pub fn next_instrument_id(&self) -> InstrumentId {
        InstrumentId(
            self.instruments
                .iter()
                .map(|i| i.id.0 + 1)
                .max()
                .unwrap_or(0),
        )
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
            instruments: vec![InstrumentState::new(InstrumentId(0), "Rhodes".into())],
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
    fn a_document_written_before_instruments_still_loads() {
        let mut state = fixture();
        state.instruments.clear();
        let mut json = serde_json::to_value(&state).unwrap();
        // Exactly what a pre-instrument daemon wrote: no key at all.
        json.as_object_mut().unwrap().remove("instruments");
        let back: MixerState = serde_json::from_value(json).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn a_document_round_trips_through_toml_with_its_instruments() {
        // toml refuses a value after a table, so `instruments` sitting
        // between the other arrays and `master` is load-bearing ordering,
        // not taste. Serializing here is what would catch a reorder.
        let state = fixture();
        let back: MixerState = toml::from_str(&toml::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn next_instrument_id_never_reuses_an_id() {
        let mut state = fixture();
        state
            .instruments
            .push(InstrumentState::new(InstrumentId(1), "Bass".into()));
        assert_eq!(state.next_instrument_id(), InstrumentId(2));
        // Removing the first must not hand its id to the next instrument:
        // a strip still patched to it would silently adopt the new one.
        state.instruments.remove(0);
        assert_eq!(state.next_instrument_id(), InstrumentId(2));
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
