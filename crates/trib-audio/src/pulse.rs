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

use crate::assembler::InputShared;
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
        .filter_map(|source| {
            let name = source.get("name")?.as_str()?;
            if name.ends_with(".monitor") {
                return None;
            }
            let description = source.get("description")?.as_str()?;
            let spec = source.get("sample_specification")?.as_str()?;
            Some(PulseSource {
                name: name.to_owned(),
                description: description.to_owned(),
                channels: parse_channels(spec)?,
                default: name == default_name,
            })
        })
        .collect();
    Some(sources)
}

/// "s16le 2ch 32000Hz" → 2.
fn parse_channels(spec: &str) -> Option<u16> {
    spec.split_whitespace()
        .find_map(|token| token.strip_suffix("ch"))
        .and_then(|n| n.parse().ok())
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
        let mut command = Command::new("parec");
        command
            .arg("--raw")
            .arg("--format=float32le")
            .arg(format!("--rate={sample_rate}"))
            .arg(format!("--channels={channels}"))
            .arg("--latency-msec=20")
            .arg("--client-name=tribd")
            .stdout(Stdio::piped())
            // parec is silent on success with --raw; on failure its stderr
            // is the only diagnostic, so let it reach the daemon's log.
            .stderr(Stdio::inherit())
            .stdin(Stdio::null());
        if let Some(source) = source {
            command.arg(format!("--device={source}"));
        }
        let mut child = command
            .spawn()
            .map_err(|e| AudioError::Device(format!("parec: {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AudioError::Device("parec gave no stdout".into()))?;

        std::thread::Builder::new()
            .name("trib-parec-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let mut carry: Vec<u8> = Vec::with_capacity(4);
                loop {
                    match stdout.read(&mut buf) {
                        // EOF: the child was killed (close) or the server
                        // died — either way this capture is over.
                        Ok(0) => break,
                        Ok(n) => {
                            // Samples may split across reads; carry the
                            // partial tail bytes to the next chunk.
                            let mut bytes = carry.clone();
                            bytes.extend_from_slice(&buf[..n]);
                            let whole = bytes.len() - bytes.len() % 4;
                            for sample in bytes[..whole].chunks_exact(4) {
                                let value =
                                    f32::from_le_bytes(sample.try_into().expect("4-byte chunk"));
                                let _ = tx.push(value);
                            }
                            carry.clear();
                            carry.extend_from_slice(&bytes[whole..]);
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
      {"name":"alsa_input.usb-046d_C922.analog-stereo","description":"C922 Pro Stream Webcam Analog Stereo","sample_specification":"s16le 2ch 32000Hz"},
      {"name":"alsa_input.usb-Dock.mono-fallback","description":"ThinkPad Thunderbolt 4 Dock USB Audio Mono","sample_specification":"s16le 1ch 48000Hz"}
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
}
