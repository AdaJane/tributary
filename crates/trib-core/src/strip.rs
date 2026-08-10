use serde::{Deserialize, Serialize};

use crate::eq::ChannelEq;
use crate::id::{BusId, StripId};
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

/// A hardware input patched into a strip: a device (by OS name) and a
/// channel within it. `device: None` means the system default input — the
/// shape every pre-multi-device manifest carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InputAssign {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub device_channel: u16,
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
        strip.input = Some(InputAssign {
            device: None,
            device_channel: 3,
        });
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
        assert_eq!(assign.device, None);
        assert_eq!(assign.device_channel, 2);
        // And a default-device patch serializes back WITHOUT the field, so
        // new autosaves still open under an old daemon.
        assert_eq!(
            serde_json::to_string(&assign).unwrap(),
            r#"{"device_channel":2}"#
        );
    }

    #[test]
    fn a_named_device_patch_round_trips_with_its_name() {
        let assign = InputAssign {
            device: Some("Scarlett 18i20 USB".into()),
            device_channel: 3,
        };
        let json = serde_json::to_string(&assign).unwrap();
        assert_eq!(
            json,
            r#"{"device":"Scarlett 18i20 USB","device_channel":3}"#
        );
        let back: InputAssign = serde_json::from_str(&json).unwrap();
        assert_eq!(back, assign);
    }
}
