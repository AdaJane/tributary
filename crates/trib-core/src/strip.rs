use serde::{Deserialize, Serialize};

use crate::eq::ChannelEq;
use crate::id::{BusId, InstrumentId, StripId};
use crate::{FADER_MIN_DB, ParseEnumError};

/// Where an aux send taps the strip signal. Pre-fader for monitor-style
/// sends, post-fader for FX (the classic default: FX level follows the mix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SendTap {
    PreFader,
    PostFader,
}

impl SendTap {
    pub const ALL: [SendTap; 2] = [SendTap::PreFader, SendTap::PostFader];

    pub const fn as_str(self) -> &'static str {
        match self {
            SendTap::PreFader => "pre_fader",
            SendTap::PostFader => "post_fader",
        }
    }
}

impl std::str::FromStr for SendTap {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SendTap::ALL
            .into_iter()
            .find(|tap| tap.as_str() == s)
            .ok_or_else(|| ParseEnumError {
                kind: "send tap",
                value: s.to_owned(),
            })
    }
}

/// Where a strip's main (post-fader, post-pan) signal goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteTarget {
    Master,
    Bus { id: BusId },
}

/// What feeds a strip: a hardware input (a device by OS name, plus a
/// channel within it) or one side of a virtual instrument.
///
/// `Device { device: None }` means the system default input — the shape
/// every pre-multi-device manifest carries.
///
/// The wire form is **untagged**, and that is deliberate: a device patch
/// serializes to exactly the bytes it always has, so a manifest written by
/// this daemon still opens under an older one. Only instrument patches —
/// which an older daemon could not render anyway — carry a shape it has
/// never seen. Instruments live in their own channel space rather than
/// borrowing a reserved device name, so device identity, its rename
/// reconciliation and its slot allocator never learn they exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum InputAssign {
    Device {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        device_channel: u16,
    },
    Instrument {
        instrument: InstrumentId,
        channel: u16,
    },
}

impl InputAssign {
    /// A hardware patch. `device: None` = the system default input.
    pub fn device(device: Option<String>, channel: u16) -> Self {
        InputAssign::Device {
            device,
            device_channel: channel,
        }
    }

    /// A patch onto one channel of a virtual instrument (0 = left).
    pub fn instrument(instrument: InstrumentId, channel: u16) -> Self {
        InputAssign::Instrument {
            instrument,
            channel,
        }
    }

    /// The device this patch names, if it is a hardware patch at all.
    /// `Some(None)` is the system default; `None` means "not hardware".
    pub fn device_name(&self) -> Option<Option<&str>> {
        match self {
            InputAssign::Device { device, .. } => Some(device.as_deref()),
            InputAssign::Instrument { .. } => None,
        }
    }

    /// The instrument this patch names, if any.
    pub fn instrument_id(&self) -> Option<InstrumentId> {
        match self {
            InputAssign::Instrument { instrument, .. } => Some(*instrument),
            InputAssign::Device { .. } => None,
        }
    }

    /// Channel within whatever the patch names.
    pub fn channel(&self) -> u16 {
        match self {
            InputAssign::Device { device_channel, .. } => *device_channel,
            InputAssign::Instrument { channel, .. } => *channel,
        }
    }
}

/// Hand-written so the two shapes cannot be confused and a patch naming
/// both — or neither — is a loud error rather than a silent pick. Serde's
/// `untagged` derive would try the variants in order and ignore whatever
/// fields it did not use, which is exactly the failure this type exists to
/// prevent.
impl<'de> Deserialize<'de> for InputAssign {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            device: Option<String>,
            #[serde(default)]
            device_channel: Option<u16>,
            #[serde(default)]
            instrument: Option<InstrumentId>,
            #[serde(default)]
            channel: Option<u16>,
        }

        let wire = Wire::deserialize(deserializer)?;
        match (
            wire.instrument,
            wire.device.is_some() || wire.device_channel.is_some(),
        ) {
            (Some(_), true) => Err(serde::de::Error::custom(
                "an input patch names either a device or an instrument, not both",
            )),
            (Some(instrument), false) => {
                let channel = wire.channel.ok_or_else(|| {
                    serde::de::Error::custom("an instrument patch needs a `channel`")
                })?;
                Ok(InputAssign::instrument(instrument, channel))
            }
            (None, _) => {
                if wire.channel.is_some() {
                    return Err(serde::de::Error::custom(
                        "`channel` belongs to an instrument patch; a device patch uses `device_channel`",
                    ));
                }
                let device_channel = wire.device_channel.ok_or_else(|| {
                    serde::de::Error::custom(
                        "an input patch needs `device_channel` or `instrument`",
                    )
                })?;
                Ok(InputAssign::device(wire.device, device_channel))
            }
        }
    }
}

/// One aux send from a strip toward a bus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SendState {
    pub dest: BusId,
    pub level_db: f32,
    pub tap: SendTap,
}

/// One channel strip, top to bottom like the panel: input, gain, EQ, sends,
/// pan, fader, switches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StripState {
    pub id: StripId,
    pub name: String,
    pub input: Option<InputAssign>,
    /// Input trim, pre-EQ.
    pub gain_db: f32,
    pub eq: ChannelEq,
    pub sends: Vec<SendState>,
    /// Constant-power pan, -1.0 (hard left) .. 1.0 (hard right).
    pub pan: f32,
    pub fader_db: f32,
    pub mute: bool,
    pub pfl: bool,
    pub record_arm: bool,
    pub route_to: RouteTarget,
}

impl StripState {
    /// A fresh strip: fader down. A newly-patched line must never blast the
    /// room — level check rides PFL, then the fader comes up.
    pub fn new(id: StripId, name: String) -> Self {
        StripState {
            id,
            name,
            input: None,
            gain_db: 0.0,
            eq: ChannelEq::default(),
            sends: Vec::new(),
            pan: 0.0,
            fader_db: FADER_MIN_DB,
            mute: false,
            pfl: false,
            record_arm: false,
            route_to: RouteTarget::Master,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_agrees_with_serde_for_every_tap() {
        for tap in SendTap::ALL {
            let json = serde_json::to_string(&tap).unwrap();
            assert_eq!(json, format!("\"{}\"", tap.as_str()));
            assert_eq!(json.trim_matches('"').parse::<SendTap>().unwrap(), tap);
        }
    }

    #[test]
    fn route_target_wire_shape_is_tagged() {
        let master = serde_json::to_value(RouteTarget::Master).unwrap();
        assert_eq!(master, serde_json::json!({ "kind": "master" }));
        let bus = serde_json::to_value(RouteTarget::Bus { id: BusId(2) }).unwrap();
        assert_eq!(bus, serde_json::json!({ "kind": "bus", "id": 2 }));
    }

    #[test]
    fn a_fresh_strip_starts_with_the_fader_down() {
        let strip = StripState::new(StripId(1), "Vocal".into());
        assert_eq!(strip.fader_db, FADER_MIN_DB);
        assert!(!strip.mute && !strip.pfl && !strip.record_arm);
        assert_eq!(strip.route_to, RouteTarget::Master);
        assert!(strip.input.is_none());
    }

    #[test]
    fn strip_round_trips_through_json() {
        let mut strip = StripState::new(StripId(4), "Gtr".into());
        strip.input = Some(InputAssign::device(None, 3));
        strip.sends.push(SendState {
            dest: BusId(0),
            level_db: -12.0,
            tap: SendTap::PostFader,
        });
        let back: StripState =
            serde_json::from_str(&serde_json::to_string(&strip).unwrap()).unwrap();
        assert_eq!(back, strip);
    }

    #[test]
    fn a_pre_multi_device_patch_still_deserializes_as_the_default_device() {
        // The exact shape every pre-existing manifest and old client carries.
        let assign: InputAssign = serde_json::from_str(r#"{"device_channel":2}"#).unwrap();
        assert_eq!(assign.device_name(), Some(None));
        assert_eq!(assign.channel(), 2);
        // And a default-device patch serializes back WITHOUT the field, so
        // new autosaves still open under an old daemon.
        assert_eq!(
            serde_json::to_string(&assign).unwrap(),
            r#"{"device_channel":2}"#
        );
    }

    #[test]
    fn a_named_device_patch_round_trips_with_its_name() {
        let assign = InputAssign::device(Some("Scarlett 18i20 USB".into()), 3);
        let json = serde_json::to_string(&assign).unwrap();
        assert_eq!(
            json,
            r#"{"device":"Scarlett 18i20 USB","device_channel":3}"#
        );
        let back: InputAssign = serde_json::from_str(&json).unwrap();
        assert_eq!(back, assign);
    }

    #[test]
    fn an_instrument_patch_round_trips_as_its_own_shape() {
        let assign = InputAssign::instrument(InstrumentId(3), 1);
        let json = serde_json::to_string(&assign).unwrap();
        assert_eq!(json, r#"{"instrument":3,"channel":1}"#);
        let back: InputAssign = serde_json::from_str(&json).unwrap();
        assert_eq!(back, assign);
        assert_eq!(back.instrument_id(), Some(InstrumentId(3)));
        assert_eq!(back.device_name(), None);
        assert_eq!(back.channel(), 1);
    }

    #[test]
    fn a_patch_naming_both_a_device_and_an_instrument_is_refused() {
        // Silently preferring one would make a mis-migrated manifest patch
        // the wrong source, which reads as an unexplainable silent strip.
        let err = serde_json::from_str::<InputAssign>(
            r#"{"device_channel":2,"instrument":1,"channel":0}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn a_patch_naming_neither_is_refused() {
        let err = serde_json::from_str::<InputAssign>(r#"{}"#).unwrap_err();
        assert!(err.to_string().contains("device_channel"), "{err}");
    }

    #[test]
    fn an_instrument_patch_without_a_channel_is_refused() {
        let err = serde_json::from_str::<InputAssign>(r#"{"instrument":1}"#).unwrap_err();
        assert!(err.to_string().contains("channel"), "{err}");
    }
}
