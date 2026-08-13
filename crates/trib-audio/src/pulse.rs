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
    /// The source's own channel map ("aux0,aux1,…"). Load-bearing: the
    /// capture opens in these exact positions, because the server routes
    /// by position NAME and its synthesised default map drops most of a
    /// pro-audio source's channels. See `parec_args`.
    pub channel_map: Option<String>,
    /// The server's mute flag. Nothing else in the capture path can see
    /// it: a muted source enumerates, opens, streams, and delivers
    /// silence, reporting no error at any step.
    pub muted: bool,
    /// The QUIETEST channel's volume as a percentage of unity — the
    /// quietest and not the average, because one attenuated channel is the
    /// one that will surprise you. None = the server did not say.
    pub volume_percent: Option<u32>,
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
        // Absent is not muted: the raw-ALSA shape carries neither key, and
        // guessing "muted" there would condemn every healthy device.
        muted: source
            .get("mute")
            .and_then(|m| m.as_bool())
            .unwrap_or(false),
        volume_percent: parse_volume(source.get("volume")),
    })
}

/// "s16le 2ch 32000Hz" → 2.
fn parse_channels(spec: &str) -> Option<u16> {
    spec.split_whitespace()
        .find_map(|token| token.strip_suffix("ch"))
        .and_then(|n| n.parse().ok())
}

/// PulseAudio's unity volume: `value` 65536 is 100 %, 0 dB.
const PA_VOLUME_NORM: u64 = 65_536;

/// The quietest channel of a `pactl` volume map, as a percentage of unity.
///
/// Shape: `{"aux0": {"value": 65536, …}, "aux1": {…}}`. An empty or
/// unparseable map is None rather than 0 — "the server didn't say" and
/// "the server said silence" are different answers.
fn parse_volume(volume: Option<&serde_json::Value>) -> Option<u32> {
    volume?
        .as_object()?
        .values()
        .filter_map(|channel| channel.get("value")?.as_u64())
        .map(|value| (value * 100 / PA_VOLUME_NORM) as u32)
        .min()
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
/// `i < channels`. The server routes by channel *position name*: absent a
/// map it synthesises one for `channels` (a 10-channel request yields
/// `front-left, front-left-of-center, front-center, front-right,
/// front-right-of-center, rear-center, aux0, aux1, aux2, aux3`) and matches
/// the source into it by name. A pro-audio source is mapped `aux0…auxN-1`
/// and barely intersects that, so inputs arrive scattered or silent.
///
/// `--channel-map` is what actually holds the contract up: asking for the
/// SOURCE's own positions makes the match an identity. `--no-remap` was
/// relied on for this and is NOT enough — **pipewire-pulse ignores it**.
/// Measured on a UMC1820 (10ch, `aux0…aux9`): with `--no-remap` alone the
/// stream still negotiated the synthesised positional map, six channels
/// arrived as digital zero and the microphone on input 1 was dropped
/// entirely — hardware meters lit, every console meter dead. With the map
/// passed explicitly the same input read -4.7 dBFS. Both flags stay:
/// `--no-remap` is correct on real PulseAudio, and `--no-remix` stops
/// unmatched positions being invented from their neighbours.
fn parec_args(
    source: Option<&str>,
    channels: u16,
    sample_rate: u32,
    channel_map: Option<&str>,
) -> Vec<String> {
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
    // Only when the server told us the map, and only when it describes the
    // width we are opening — a stale or mismatched map would route worse
    // than the synthesised one.
    .chain(
        channel_map
            .filter(|map| map.split(',').count() == usize::from(channels))
            .map(|map| format!("--channel-map={map}")),
    )
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
        channel_map: Option<&str>,
        mut tx: Producer<f32>,
        shared: Arc<InputShared>,
    ) -> Result<ParecCapture, AudioError> {
        // A zero-channel capture is a control-side bug, not a condition:
        // it would divide by zero framing the stream.
        if channels == 0 {
            return Err(AudioError::Device("capture needs a channel".into()));
        }
        let mut child = Command::new("parec")
            .args(parec_args(source, channels, sample_rate, channel_map))
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
    fn a_muted_or_attenuated_source_says_so() {
        // The failure this exists for: a muted source enumerates, opens and
        // streams exactly like a healthy one, so nothing downstream can
        // tell the console why every meter reads silence.
        let json = r#"[
          {"name":"muted","description":"Muted","sample_specification":"s16le 2ch 48000Hz",
           "mute":true,
           "volume":{"front-left":{"value":65536},"front-right":{"value":65536}}},
          {"name":"lopsided","description":"One channel down","sample_specification":"s16le 2ch 48000Hz",
           "mute":false,
           "volume":{"front-left":{"value":65536},"front-right":{"value":32768}}}
        ]"#;
        let sources = parse_sources(json, "").unwrap();
        assert!(sources[0].muted);
        assert_eq!(sources[0].volume_percent, Some(100));
        assert!(!sources[1].muted);
        assert_eq!(
            sources[1].volume_percent,
            Some(50),
            "the quietest channel, not the average: an input is only as \
             audible as its quietest channel"
        );
    }

    #[test]
    fn a_source_that_reports_no_volume_is_not_assumed_muted() {
        // The raw-ALSA fixture carries neither key.
        let sources = parse_sources(FIXTURE, "").unwrap();
        assert!(!sources[0].muted);
        assert_eq!(
            sources[0].volume_percent, None,
            "unknown is not zero — a device that never reported a volume \
             must not read as silenced"
        );
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
    fn capture_opens_in_the_sources_own_channel_positions() {
        // The bug this exists for, measured on a UMC1820: `--no-remap` is
        // IGNORED by pipewire-pulse, so the stream negotiated the server's
        // synthesised 10-channel surround map, six channels arrived as
        // digital zero and input 1 vanished. Naming the source's own
        // positions makes the server's by-name match an identity.
        let map = "aux0,aux1,aux2,aux3,aux4,aux5,aux6,aux7,aux8,aux9";
        let args = parec_args(Some("umc.pro-input-0"), 10, 48_000, Some(map));
        assert!(
            args.contains(&format!("--channel-map={map}")),
            "without this the server invents front-left/front-left-of-center/…              and drops every position the source does not share: {args:?}"
        );
    }

    #[test]
    fn a_channel_map_that_does_not_match_the_width_is_refused() {
        // A stale map would route worse than the synthesised one, so the
        // width is the gate: mismatch means fall back rather than guess.
        let args = parec_args(Some("dev"), 10, 48_000, Some("aux0,aux1"));
        assert!(!args.iter().any(|a| a.starts_with("--channel-map=")));
        let args = parec_args(Some("dev"), 2, 48_000, Some("front-left,front-right"));
        assert!(args.contains(&"--channel-map=front-left,front-right".to_owned()));
    }

    #[test]
    fn an_unknown_channel_map_is_simply_omitted() {
        // Raw-ALSA and pre-map enumerations carry none; the flags below
        // remain the best available guarantee there.
        let args = parec_args(Some("dev"), 4, 48_000, None);
        assert!(!args.iter().any(|a| a.starts_with("--channel-map=")));
        assert!(args.iter().any(|a| a == "--no-remap"));
    }

    #[test]
    fn capture_routes_channels_by_index_not_by_position_name() {
        let args = parec_args(Some("alsa_input.usb-umc1820.pro-audio"), 18, 48_000, None);
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
        let args = parec_args(None, 2, 44_100, None);
        assert!(
            !args.iter().any(|a| a.starts_with("--device=")),
            "parec with no --device tracks the server default live"
        );
    }
}
