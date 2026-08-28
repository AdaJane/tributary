//! Layered configuration: base TOML file ← `TRIB__SECTION__KEY` env overrides.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub server: Server,
    pub audio: Audio,
    pub projects: Projects,
    pub soundfonts: Soundfonts,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    pub bind: String,
    /// Non-loopback browser origins allowed by CORS and the WS handshake
    /// origin check (loopback passes on any port without configuration).
    pub cors_origins: Vec<String>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            bind: "127.0.0.1:4600".into(),
            cors_origins: Vec::new(),
        }
    }
}

/// Which device layer owns the audio hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioLayer {
    /// PipeWire/PulseAudio alongside everything else on the machine.
    ///
    /// The only safe default: taking a card exclusively would silence the
    /// user's music, and a daemon cannot tell "I am an appliance" from "I
    /// am running on somebody's laptop" by looking.
    #[default]
    Shared,
    /// One ALSA `hw:` card, capture and playback on one clock. The
    /// appliance posture — no server in the path, and nothing else on the
    /// box can make a sound.
    Exclusive,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Audio {
    pub sample_rate: u32,
    pub block_size: usize,
    pub layer: AudioLayer,
    /// The card the exclusive layer takes ("hw:1"). None = the first
    /// card that opens duplex at the engine rate, in ALSA index order.
    pub device: Option<String>,
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            sample_rate: 48_000,
            block_size: 256,
            layer: AudioLayer::default(),
            device: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Projects {
    pub root: PathBuf,
}

impl Default for Projects {
    fn default() -> Self {
        Projects {
            root: "projects".into(),
        }
    }
}

/// Where SoundFont files live, and how big one may be.
///
/// Deliberately its own root rather than a sibling of `recording.toml`:
/// that one resolves beside the config file, which packaging installs as
/// `/etc/tributary/` conf-files, and hundreds of megabytes of samples do
/// not belong in `/etc`. It is also NOT under the recording destination —
/// that moves to whatever USB stick is mounted, and an instrument would
/// lose its soundfont every time the destination changed.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Soundfonts {
    pub root: PathBuf,
    /// Ceiling on an uploaded file. `rustysynth` holds all sample data
    /// resident, so this is a memory budget wearing a disk-shaped hat: a
    /// 148 MB General MIDI set is an OOM kill on a 2 GB Pi 4, not a slow
    /// load.
    pub max_bytes: u64,
}

impl Default for Soundfonts {
    fn default() -> Self {
        Soundfonts {
            root: "soundfonts".into(),
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

/// The engine rates the DSP is validated for — one whitelist for boot
/// config and the recording prefs API alike.
pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("invalid config: {0}")]
    Invalid(String),
}

/// Load and validate. A missing file is the defaults, not an error — a fresh
/// checkout must boot.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let settings: Settings = config::Config::builder()
        .add_source(config::File::from(path).required(false))
        .add_source(
            config::Environment::with_prefix("TRIB")
                .prefix_separator("__")
                .separator("__"),
        )
        .build()?
        .try_deserialize()?;
    settings.validate()?;
    Ok(settings)
}

impl Settings {
    /// Cross-field invariants, checked on every load so a bad edit is refused
    /// before it reaches the engine.
    pub fn validate(&self) -> Result<(), SettingsError> {
        self.server
            .bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| SettingsError::Invalid(format!("server.bind: {}", self.server.bind)))?;
        if !SAMPLE_RATES.contains(&self.audio.sample_rate) {
            return Err(SettingsError::Invalid(format!(
                "audio.sample_rate must be one of {SAMPLE_RATES:?}"
            )));
        }
        let block = self.audio.block_size;
        if !block.is_power_of_two() || !(32..=2048).contains(&block) {
            return Err(SettingsError::Invalid(
                "audio.block_size must be a power of two in 32..=2048".into(),
            ));
        }
        if self.soundfonts.max_bytes == 0 {
            return Err(SettingsError::Invalid(
                "soundfonts.max_bytes must be greater than zero".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_yields_the_defaults() {
        let settings = load(Path::new("/nonexistent/tribd.toml")).unwrap();
        assert_eq!(settings.server.bind, "127.0.0.1:4600");
        assert_eq!(settings.audio.sample_rate, 48_000);
        assert_eq!(settings.audio.block_size, 256);
    }

    #[test]
    fn an_unknown_key_is_refused_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tribd.toml");
        std::fs::write(&path, "[audio]\nsample_rats = 48000\n").unwrap();
        assert!(load(&path).is_err(), "typos must not silently vanish");
    }

    #[test]
    fn validate_rejects_odd_block_sizes() {
        let mut settings = Settings::default();
        settings.audio.block_size = 300;
        assert!(settings.validate().is_err());
        settings.audio.block_size = 16_384;
        assert!(settings.validate().is_err());
        settings.audio.block_size = 128;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn validate_rejects_unparseable_bind() {
        let mut settings = Settings::default();
        settings.server.bind = "not-an-addr".into();
        assert!(settings.validate().is_err());
    }
}
