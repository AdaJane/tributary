use serde::{Deserialize, Serialize};

use crate::id::InstrumentId;

/// Voices an instrument may hold at once. Notes past it steal the oldest.
pub const DEFAULT_POLYPHONY: u16 = 32;

/// One MIDI message as it was applied, stamped with where in the take it
/// happened.
///
/// Lives in the pure domain because the engine produces it and the project
/// writer consumes it, and those two crates share nothing else — the audio
/// rings get away with `f32`, but a struct needs a home both can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapturedMidi {
    /// Samples since the take started — NOT a block counter, which is a
    /// different unit and would put every event in the wrong place by a
    /// factor of the block size.
    pub sample: u64,
    /// Which rack slot produced it.
    pub instrument: u16,
    /// Status nibble, channel already stripped.
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

/// One mixer channel of a split instrument: a named slice of the keyboard.
///
/// This is how a drum kit reaches the desk as separate faders. A SoundFont
/// kit is one MIDI channel with a different drum on every key, so the only
/// axis that separates a kick from a snare is the key number.
///
/// A split is **mono**, deliberately. A drum is a point source you pan
/// yourself, and the alternative — two strips per piece — turns a five-piece
/// kit into ten channels of desk to manage before anyone has played a note.
/// An unsplit instrument stays stereo, because a piano's stereo image is
/// the thing you actually want.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InstrumentSplit {
    pub name: String,
    /// Inclusive key ranges this channel answers to.
    ///
    /// A list, not one range, because the General MIDI drum map
    /// **interleaves** toms and hi-hats — 41 tom, 42 hat, 43 tom, 44 hat,
    /// 45 tom, 46 hat. A single contiguous range cannot put the toms on
    /// one fader and the hats on another, which is the first thing anyone
    /// mixing drums wants.
    pub ranges: Vec<(u8, u8)>,
}

impl InstrumentSplit {
    pub fn new(name: impl Into<String>, ranges: &[(u8, u8)]) -> Self {
        InstrumentSplit {
            name: name.into(),
            ranges: ranges.to_vec(),
        }
    }

    pub fn covers(&self, key: u8) -> bool {
        self.ranges
            .iter()
            .any(|(lo, hi)| (*lo..=*hi).contains(&key))
    }
}

/// The General MIDI drum map, grouped the way a desk is.
///
/// Not every GM key on its own fader — the groups an engineer actually
/// reaches for. Keys no split covers are silent, and the console has to say
/// so rather than leave someone hitting a pad that does nothing.
pub fn gm_drum_splits() -> Vec<InstrumentSplit> {
    vec![
        InstrumentSplit::new("Kick", &[(35, 36)]),
        InstrumentSplit::new("Snare", &[(37, 40)]),
        // Interleaved with the hats, hence the ranges.
        InstrumentSplit::new("Toms", &[(41, 41), (43, 43), (45, 45), (47, 48), (50, 50)]),
        InstrumentSplit::new("HiHat", &[(42, 42), (44, 44), (46, 46)]),
        InstrumentSplit::new("Cymbals", &[(49, 49), (51, 59)]),
        InstrumentSplit::new("Percussion", &[(60, 81)]),
    ]
}

/// A virtual instrument: a SoundFont, a preset within it, and the MIDI
/// traffic it answers to.
///
/// Part of the console document, so it persists in `project.toml` and
/// reaches every client through the mixer snapshot. Note what is *not*
/// here: whether the soundfont actually loaded, how many presets it has,
/// whether its port is connected. Those are facts about the machine, not
/// about the desk, and they travel on the instruments report the way a
/// device's status travels on the devices report — the same rule that keeps
/// the transport out of the reducer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InstrumentState {
    pub id: InstrumentId,
    pub name: String,
    /// A file name within the soundfont library — never a path.
    /// `project.toml` travels to a USB stick and opens on another box, so a
    /// path would be a promise about a machine the session may never see
    /// again. `None` = nothing chosen yet, which is silence with a reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soundfont: Option<String>,
    pub bank: u16,
    pub program: u8,
    /// MIDI source port, by name. `None` = bound to nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// `None` = omni: every channel on that port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_channel: Option<u8>,
    pub polyphony: u16,
    /// `rustysynth`'s own reverb and chorus. Off by default: the console
    /// already ships an FX 1 reverb send, and stacking a second one costs a
    /// Pi real CPU to sound worse.
    pub effects: bool,
    /// Per-drum (or per-region) mixer channels. Empty = one stereo output,
    /// which is what a piano wants; non-empty = one mono channel per split,
    /// which is what a kit wants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splits: Vec<InstrumentSplit>,
}

impl InstrumentState {
    /// Whether this instrument listens to `port` on `channel`.
    ///
    /// The control-side statement of the rule the rack enforces by index:
    /// `trib_engine`'s `MidiBinding::accepts` is this predicate after port
    /// names have been resolved to `u8`s for the audio thread. Stated here
    /// so the MIDI echo — which works from the document and never sees an
    /// index — cannot disagree with what actually sounded. `trib-engine`
    /// pins the two together with a test.
    ///
    /// An instrument bound to no port accepts nothing: silence with a
    /// reason, rather than omni by accident.
    pub fn accepts(&self, port: &str, channel: u8) -> bool {
        self.port.as_deref() == Some(port) && self.midi_channel.is_none_or(|c| c == channel)
    }

    /// A fresh instrument: named, but pointed at nothing. It renders
    /// silence until a soundfont and a port are chosen, and the report says
    /// which of the two is missing.
    pub fn new(id: InstrumentId, name: String) -> Self {
        InstrumentState {
            id,
            name,
            soundfont: None,
            bank: 0,
            program: 0,
            port: None,
            midi_channel: None,
            polyphony: DEFAULT_POLYPHONY,
            effects: false,
            splits: Vec::new(),
        }
    }

    /// Mixer channels this instrument occupies: one mono channel per split,
    /// or the stereo pair when it is unsplit.
    pub fn channels(&self) -> usize {
        if self.splits.is_empty() {
            2
        } else {
            self.splits.len()
        }
    }

    /// Rack slots this instrument occupies — one synthesiser per split, or
    /// a single stereo one. Differs from [`channels`] only because the
    /// unsplit case is one slot writing two channels.
    pub fn channels_slots(&self) -> usize {
        self.splits.len().max(1)
    }

    /// What each channel should be called on the tape.
    ///
    /// The names the "add all channels" button writes, so the desk reads
    /// "Kit Kick", "Kit Snare" rather than "Inst 1 ch 3".
    pub fn channel_names(&self) -> Vec<String> {
        if self.splits.is_empty() {
            vec![format!("{} L", self.name), format!("{} R", self.name)]
        } else {
            self.splits
                .iter()
                .map(|split| format!("{} {}", self.name, split.name))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_instrument_points_at_nothing_and_carries_no_effects() {
        let inst = InstrumentState::new(InstrumentId(0), "Rhodes".into());
        assert_eq!(inst.soundfont, None);
        assert_eq!(inst.port, None);
        assert_eq!(inst.midi_channel, None);
        assert_eq!(inst.polyphony, DEFAULT_POLYPHONY);
        assert!(!inst.effects);
    }

    #[test]
    fn an_unbound_instrument_omits_the_fields_it_has_not_chosen() {
        let inst = InstrumentState::new(InstrumentId(1), "Pad".into());
        assert_eq!(
            serde_json::to_string(&inst).unwrap(),
            r#"{"id":1,"name":"Pad","bank":0,"program":0,"polyphony":32,"effects":false}"#
        );
    }

    #[test]
    fn a_bound_instrument_round_trips_through_json() {
        let inst = InstrumentState {
            id: InstrumentId(2),
            name: "Rhodes".into(),
            soundfont: Some("piano.sf2".into()),
            bank: 0,
            program: 4,
            port: Some("nanoKEY2 MIDI 1".into()),
            midi_channel: Some(9),
            polyphony: 64,
            effects: true,
            splits: Vec::new(),
        };
        let back: InstrumentState =
            serde_json::from_str(&serde_json::to_string(&inst).unwrap()).unwrap();
        assert_eq!(back, inst);
    }
}
