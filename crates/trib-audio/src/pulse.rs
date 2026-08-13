//! PulseAudio/PipeWire sources: the SAME input list the system sound
//! settings show, with friendly names, opened BY NAME. Enumeration rides
//! `pactl --format=json` and capture rides a `parec` child per source —
//! textual tools from the same package, no native bindings, and the
//! server resamples to the engine rate for us. Systems without a Pulse
//! server fall back to raw ALSA enumeration (the caller's job).

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rtrb::Producer;

use crate::assembler::{InputShared, push_frames};
use crate::backend::AudioError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseSource {
    /// Stable identity ("alsa_input.usb-046d_C922…") — what patches store.
    pub name: String,
    /// What the system menu prints ("C922 Pro Stream Webcam Analog Stereo").
    pub description: String,
    pub channels: u16,
    /// The server's default source — what `device: null` patches follow.
    pub default: bool,
    /// The card this source belongs to ("alsa_card.usb-…"), when the server
    /// says. Profiles are selected on the CARD, never on the source.
    pub card: Option<String>,
    /// The source's own channel map ("aux0,aux1,…"). Capture routes by
    /// INDEX, so this is only a diagnostic: it records what those indices
    /// mean on the hardware.
    pub channel_map: Option<String>,
}

/// A sound card and the profiles it can be switched between.
///
/// A card's ACTIVE profile decides how many channels its sources expose. An
/// interface with eighteen inputs parked in a stereo profile genuinely has
/// no other channels to enumerate — the missing inputs are not a bug in the
/// capture path, they do not exist until the profile changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseCard {
    /// `alsa_card.usb-…` — what `pactl set-card-profile` takes.
    pub name: String,
    pub active_profile: String,
    /// Capture-capable profiles only, in the order the system sound panel
    /// would list them.
    pub profiles: Vec<PulseProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseProfile {
    /// `pro-audio` — the identity handed back to select it.
    pub name: String,
    /// "Pro Audio" — what the system sound panel prints.
    pub description: String,
}

/// The system's input sources, or None when no Pulse server answers.
pub fn enumerate_sources() -> Option<Vec<PulseSource>> {
    let list = Command::new("pactl")
        .args(["--format=json", "list", "sources"])
        .output()
        .ok()?;
    if !list.status.success() {
        return None;
    }
    let default = Command::new("pactl")
        .arg("get-default-source")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let json = String::from_utf8_lossy(&list.stdout);
    let sources = parse_sources(&json, &default)?;
    (!sources.is_empty()).then_some(sources)
}

/// Pure parse of `pactl --format=json list sources`. Monitor sources (the
/// sinks' loopbacks) are not inputs and are dropped, matching the system
/// menu.
pub fn parse_sources(json: &str, default_name: &str) -> Option<Vec<PulseSource>> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    let sources = parsed
        .as_array()?
        .iter()
        .filter(|source| !source_name(source).is_some_and(|n| n.ends_with(".monitor")))
        .filter_map(|source| {
            parse_source(source, default_name).or_else(|| {
                // Silence here used to make a device vanish from the
                // patchbay with no trace anywhere.
                tracing::warn!(
                    source = source_name(source).unwrap_or("<unnamed>"),
                    "input source dropped: pactl gave no usable name, \
                     description or sample specification"
                );
                None
            })
        })
        .collect();
    Some(sources)
}

fn source_name(source: &serde_json::Value) -> Option<&str> {
    source.get("name")?.as_str()
}

fn parse_source(source: &serde_json::Value, default_name: &str) -> Option<PulseSource> {
    let name = source_name(source)?;
    let description = source.get("description")?.as_str()?;
    let spec = source.get("sample_specification")?.as_str()?;
    Some(PulseSource {
        name: name.to_owned(),
        description: description.to_owned(),
        channels: parse_channels(spec)?,
        default: name == default_name,
        // The card is the source's `device.name` property — confusingly
        // named, but it is the `alsa_card.…` identity, not the source's.
        card: source
            .get("properties")
            .and_then(|p| p.get("device.name"))
            .and_then(|c| c.as_str())
            .map(str::to_owned),
        channel_map: source
            .get("channel_map")
            .and_then(|m| m.as_str())
            .map(str::to_owned),
    })
}

/// "s16le 2ch 32000Hz" → 2.
fn parse_channels(spec: &str) -> Option<u16> {
    spec.split_whitespace()
        .find_map(|token| token.strip_suffix("ch"))
        .and_then(|n| n.parse().ok())
}

/// The system's sound cards, or None when no Pulse server answers.
pub fn enumerate_cards() -> Option<Vec<PulseCard>> {
    let list = Command::new("pactl")
        .args(["--format=json", "list", "cards"])
        .output()
        .ok()?;
    if !list.status.success() {
        return None;
    }
    parse_cards(&String::from_utf8_lossy(&list.stdout))
}

/// Pure parse of `pactl --format=json list cards`.
///
/// Only capture-capable profiles survive: `off` and output-only profiles
/// would remove the input altogether, and an unavailable one cannot be
/// selected. Ordered by the server's own priority, so the list reads the
/// same way the system sound panel does.
pub fn parse_cards(json: &str) -> Option<Vec<PulseCard>> {
    let parsed: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(parsed.as_array()?.iter().filter_map(parse_card).collect())
}

fn parse_card(card: &serde_json::Value) -> Option<PulseCard> {
    let name = card.get("name")?.as_str()?;
    let active_profile = card.get("active_profile")?.as_str()?;
    let mut ranked: Vec<(i64, PulseProfile)> = card
        .get("profiles")?
        .as_object()?
        .iter()
        .filter(|(_, profile)| captures(profile))
        .map(|(name, profile)| {
            (
                profile
                    .get("priority")
                    .and_then(|p| p.as_i64())
                    .unwrap_or(0),
                PulseProfile {
                    name: name.clone(),
                    description: profile
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or(name)
                        .to_owned(),
                },
            )
        })
        .collect();
    ranked.sort_by(|(a_rank, a), (b_rank, b)| b_rank.cmp(a_rank).then_with(|| a.name.cmp(&b.name)));
    Some(PulseCard {
        name: name.to_owned(),
        active_profile: active_profile.to_owned(),
        profiles: ranked.into_iter().map(|(_, profile)| profile).collect(),
    })
}

/// A profile worth offering: selectable, and leaves at least one input.
fn captures(profile: &serde_json::Value) -> bool {
    profile.get("available").and_then(|a| a.as_bool()) != Some(false)
        && profile.get("sources").and_then(|s| s.as_u64()).unwrap_or(0) > 0
}

/// Put `card` into `profile`.
///
/// The sources it exposes change wholesale — names, channel counts and
/// channel maps — so every caller must re-enumerate afterwards rather than
/// trust anything it read before.
pub fn set_card_profile(card: &str, profile: &str) -> Result<(), AudioError> {
    let out = Command::new("pactl")
        .args(["set-card-profile", card, profile])
        .output()
        .map_err(|e| AudioError::Device(format!("pactl: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(AudioError::Device(format!(
        "pactl set-card-profile {card} {profile}: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// How much buffering to ask the server for. Small enough to keep the
/// patchbay responsive, large enough that a busy Pi doesn't tear frames.
const PAREC_LATENCY_MSEC: u32 = 20;

/// `--format=float32le`, so one sample is four little-endian bytes.
const SAMPLE_BYTES: usize = 4;

/// One `read` off parec's stdout.
const READ_BUF_BYTES: usize = 4096;

/// The `parec` command line for one capture.
///
/// Contract: stream channel `i` carries source channel `i` for every
/// `i < channels`. That is only true because of `--no-remap` — the default
/// is to route by channel *position name*, which makes the server
/// synthesise its own map for `channels` (an 8-channel request yields
/// `front-left, front-left-of-center, front-center, front-right, …`) and
/// match the source into it by name. A pro-audio source is mapped
/// `aux0…auxN-1` and barely intersects that, so inputs arrive scattered or
/// silent. `--no-remix` stops the unmatched positions being invented from
/// their neighbours.
fn parec_args(source: Option<&str>, channels: u16, sample_rate: u32) -> Vec<String> {
    [
        "--raw".to_owned(),
        "--format=float32le".to_owned(),
        format!("--rate={sample_rate}"),
        format!("--channels={channels}"),
        "--no-remap".to_owned(),
        "--no-remix".to_owned(),
        format!("--latency-msec={PAREC_LATENCY_MSEC}"),
        "--client-name=tribd".to_owned(),
    ]
    .into_iter()
    // No --device at all: parec then tracks the server default live.
    .chain(source.map(|s| format!("--device={s}")))
    .collect()
}

/// A running `parec` capture: the child streams raw f32 frames on stdout;
/// a reader thread pushes them into the device's ring.
pub struct ParecCapture {
    child: Child,
}

impl ParecCapture {
    /// Spawn `parec` on the named source (None = the server default) and
    /// its reader thread feeding `tx`.
    pub fn spawn(
        source: Option<&str>,
        channels: u16,
        sample_rate: u32,
        mut tx: Producer<f32>,
        shared: Arc<InputShared>,
    ) -> Result<ParecCapture, AudioError> {
        // A zero-channel capture is a control-side bug, not a condition:
        // it would divide by zero framing the stream.
        if channels == 0 {
            return Err(AudioError::Device("capture needs a channel".into()));
        }
        let mut child = Command::new("parec")
            .args(parec_args(source, channels, sample_rate))
            .stdout(Stdio::piped())
            // parec is silent on success with --raw; on failure its stderr
            // is the only diagnostic, so let it reach the daemon's log.
            .stderr(Stdio::inherit())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| AudioError::Device(format!("parec: {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AudioError::Device("parec gave no stdout".into()))?;

        let channels = usize::from(channels);
        let frame_bytes = channels * SAMPLE_BYTES;
        std::thread::Builder::new()
            .name("trib-parec-reader".into())
            .spawn(move || {
                let mut buf = [0u8; READ_BUF_BYTES];
                // Carried at FRAME granularity, not sample granularity: the
                // ring must never come to rest holding a partial frame.
                let mut carry: Vec<u8> = Vec::with_capacity(frame_bytes + READ_BUF_BYTES);
                let mut decoded: Vec<f32> = Vec::with_capacity(carry.capacity() / SAMPLE_BYTES);
                loop {
                    match stdout.read(&mut buf) {
                        // EOF: the child was killed (close) or the server
                        // died — either way this capture is over.
                        Ok(0) => break,
                        Ok(n) => {
                            carry.extend_from_slice(&buf[..n]);
                            let whole = carry.len() - carry.len() % frame_bytes;
                            decoded.clear();
                            decoded.extend(
                                carry[..whole].chunks_exact(SAMPLE_BYTES).map(|s| {
                                    f32::from_le_bytes(s.try_into().expect("4-byte sample"))
                                }),
                            );
                            let dropped = push_frames(&mut tx, &decoded, channels);
                            if dropped > 0 {
                                shared.overruns.fetch_add(dropped, Ordering::Relaxed);
                            }
                            carry.drain(..whole);
                        }
                        Err(_) => break,
                    }
                }
                shared.failed.store(true, Ordering::Relaxed);
            })
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        Ok(ParecCapture { child })
    }
}

/// Closing the capture kills the child; the reader sees EOF and exits,
/// dropping the ring producer off the audio thread.
impl Drop for ParecCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait(); // reap — no zombie per closed input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {"name":"alsa_output.pci.hdmi.monitor","description":"Monitor of HDMI Output","sample_specification":"s32le 2ch 48000Hz"},
      {"name":"alsa_input.pci.mic","description":"Meteor Lake-P HD Audio Controller Digital Microphone","sample_specification":"s32le 2ch 48000Hz"},
      {"name":"alsa_input.usb-046d_C922.analog-stereo","description":"C922 Pro Stream Webcam Analog Stereo","sample_specification":"s16le 2ch 32000Hz","channel_map":"front-left,front-right","properties":{"device.name":"alsa_card.usb-046d_C922","device.bus":"usb"}},
      {"name":"alsa_input.usb-Dock.mono-fallback","description":"ThinkPad Thunderbolt 4 Dock USB Audio Mono","sample_specification":"s16le 1ch 48000Hz"}
    ]"#;

    /// Shaped after real `pactl --format=json list cards` output.
    const CARDS: &str = r#"[
      {
        "name": "alsa_card.usb-Behringer_UMC1820",
        "active_profile": "input:analog-stereo",
        "profiles": {
          "off": {"description":"Off","sinks":0,"sources":0,"priority":0,"available":true},
          "input:analog-stereo": {"description":"Analog Stereo Input","sinks":0,"sources":1,"priority":65,"available":true},
          "output:analog-stereo": {"description":"Analog Stereo Output","sinks":1,"sources":0,"priority":60,"available":true},
          "pro-audio": {"description":"Pro Audio","sinks":1,"sources":1,"priority":1,"available":true},
          "input:unplugged": {"description":"Unplugged Input","sinks":0,"sources":1,"priority":50,"available":false}
        }
      }
    ]"#;

    #[test]
    fn parses_the_system_menu_sources_and_drops_monitors() {
        let sources = parse_sources(FIXTURE, "alsa_input.usb-046d_C922.analog-stereo").unwrap();
        assert_eq!(sources.len(), 3, "the sink monitor is not an input");
        assert_eq!(
            sources[0].description,
            "Meteor Lake-P HD Audio Controller Digital Microphone"
        );
        assert_eq!(sources[1].channels, 2);
        assert!(sources[1].default, "the C922 is the system default");
        assert_eq!(sources[2].channels, 1, "the dock input is mono");
        assert!(!sources[2].default);
    }

    #[test]
    fn garbage_json_is_none_not_a_panic() {
        assert!(parse_sources("not json", "x").is_none());
        assert!(parse_sources("{}", "x").is_none());
    }

    #[test]
    fn channel_specs_parse_defensively() {
        assert_eq!(parse_channels("s16le 2ch 32000Hz"), Some(2));
        assert_eq!(parse_channels("float32le 8ch 96000Hz"), Some(8));
        assert_eq!(parse_channels("weird spec"), None);
    }

    #[test]
    fn a_source_carries_its_card_and_channel_map() {
        let sources = parse_sources(FIXTURE, "").unwrap();
        let webcam = &sources[1];
        assert_eq!(
            webcam.card.as_deref(),
            Some("alsa_card.usb-046d_C922"),
            "the card is the source's `device.name` property, not its own name"
        );
        assert_eq!(
            webcam.channel_map.as_deref(),
            Some("front-left,front-right")
        );
        assert_eq!(sources[0].card, None, "absent properties are not an error");
    }

    #[test]
    fn a_source_with_an_unusable_spec_is_dropped_not_defaulted() {
        let json = r#"[
          {"name":"good","description":"Good","sample_specification":"s16le 2ch 48000Hz"},
          {"name":"bad","description":"Bad","sample_specification":"who knows"}
        ]"#;
        let sources = parse_sources(json, "").unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "good");
    }

    #[test]
    fn only_capture_capable_profiles_are_offered() {
        let cards = parse_cards(CARDS).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].name, "alsa_card.usb-Behringer_UMC1820");
        assert_eq!(cards[0].active_profile, "input:analog-stereo");
        let offered: Vec<&str> = cards[0].profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            offered,
            vec!["input:analog-stereo", "pro-audio"],
            "off, output-only and unavailable profiles would leave no input at all; \
             order follows the server's priority, like the system sound panel"
        );
        assert_eq!(cards[0].profiles[1].description, "Pro Audio");
    }

    #[test]
    fn card_garbage_is_none_not_a_panic() {
        assert!(parse_cards("not json").is_none());
        assert!(parse_cards("{}").is_none());
        assert_eq!(parse_cards("[]").unwrap().len(), 0);
    }

    #[test]
    fn capture_routes_channels_by_index_not_by_position_name() {
        let args = parec_args(Some("alsa_input.usb-umc1820.pro-audio"), 18, 48_000);
        assert!(
            args.iter().any(|a| a == "--no-remap"),
            "without --no-remap the server routes by position name: it \
             synthesises a map for N channels and a source's channel 1 \
             lands on stream channel 3, the rest silent"
        );
        assert!(
            args.iter().any(|a| a == "--no-remix"),
            "without --no-remix absent positions are synthesised from \
             their neighbours instead of staying silent"
        );
        assert!(args.contains(&"--channels=18".to_owned()));
        assert!(args.contains(&"--rate=48000".to_owned()));
        assert!(args.contains(&"--device=alsa_input.usb-umc1820.pro-audio".to_owned()));
    }

    #[test]
    fn the_default_source_is_followed_by_omitting_device() {
        let args = parec_args(None, 2, 44_100);
        assert!(
            !args.iter().any(|a| a.starts_with("--device=")),
            "parec with no --device tracks the server default live"
        );
    }
}
