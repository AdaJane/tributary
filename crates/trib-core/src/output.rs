//! Physical outputs: the patch bay's other half.
//!
//! An input patch is a field ON a strip because a strip has exactly one
//! input. An output patch is a row in a top-level list because a jack has
//! exactly one feed and the thing feeding it may be a bus or the master,
//! neither of which is a strip.

use serde::{Deserialize, Serialize};

use crate::id::{BusId, StripId};
use crate::strip::SendTap;

/// Output channels the patch bay may address — the sibling of the engine's
/// `MAX_OUTPUT_CHANNELS`, and the same number for the same reason: one
/// patch is one output channel, so this is the width of the plane the
/// engine assembles.
pub const MAX_OUTPUT_PATCHES: usize = 64;

/// What feeds an output channel.
///
/// The same three cases as [`crate::FaderTarget`] and [`crate::MeterKey`],
/// and a third type on purpose: those name what a level command moves and
/// what a peak describes. This names what leaves the box. They agree
/// today; nothing makes them agree tomorrow — and calling this one
/// "fader target" would put the word fader into the wire shape of a tap
/// that is deliberately pre-fader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputSource {
    Strip { id: StripId },
    Bus { id: BusId },
    Master,
}

/// A physical output channel — the identity of a patch.
///
/// One jack, at most one patch: the mirror of one strip, at most one
/// input. An output carries one signal, and summing two sources onto it
/// would make this a mixer rather than a patch bay.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OutputJack {
    /// `None` = the system default output — the meaning
    /// `InputAssign::Device { device: None }` already has on the way in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub channel: u16,
}

/// One direct out: a point in the console wired to a jack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OutputPatch {
    /// See [`OutputJack::device`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub channel: u16,
    /// Channel within the source: 0 for a strip or an aux bus, 0 or 1 for
    /// a group bus or the master.
    pub source_channel: u16,
    /// Where the signal is tapped.
    ///
    /// Pre-fader by default, which in this engine means post-gain,
    /// post-EQ, **pre-mute and pre-fader** — the same point the meter and
    /// record taps use. A direct out is for the monitor engineer, and a
    /// front-of-house fader move must not change what they get.
    pub tap: SendTap,
    /// Declared last so the emitted TOML reads scalars-then-table, the
    /// shape every other record in the manifest already has.
    pub source: OutputSource,
}

impl OutputPatch {
    pub fn new(source: OutputSource, source_channel: u16, jack: OutputJack) -> Self {
        OutputPatch {
            device: jack.device,
            channel: jack.channel,
            source_channel,
            tap: SendTap::PreFader,
            source,
        }
    }

    /// The jack this patch owns — its identity.
    pub fn jack(&self) -> OutputJack {
        OutputJack {
            device: self.device.clone(),
            channel: self.channel,
        }
    }

    /// Whether this patch owns `jack`, without cloning to find out.
    pub fn is(&self, jack: &OutputJack) -> bool {
        self.device == jack.device && self.channel == jack.channel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_on_the_default_device_omits_it_on_the_wire() {
        // Mirrors the input side, where a pre-multi-device patch writes no
        // `device` key at all: the default output is spelt by absence.
        let patch = OutputPatch::new(
            OutputSource::Master,
            1,
            OutputJack {
                device: None,
                channel: 5,
            },
        );
        assert_eq!(
            serde_json::to_string(&patch).unwrap(),
            r#"{"channel":5,"source_channel":1,"tap":"pre_fader","source":{"kind":"master"}}"#
        );
    }

    #[test]
    fn a_patch_identifies_itself_by_jack_not_by_source() {
        let jack = OutputJack {
            device: Some("interface".into()),
            channel: 3,
        };
        let patch = OutputPatch::new(OutputSource::Strip { id: StripId(7) }, 0, jack.clone());
        assert!(patch.is(&jack));
        assert_eq!(patch.jack(), jack);
        assert!(
            !patch.is(&OutputJack {
                device: None,
                channel: 3,
            }),
            "channel 3 on the default device is a different jack"
        );
    }
}
