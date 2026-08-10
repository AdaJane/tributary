//! User-editable recording preferences: the daemon-owned half of the
//! configuration. `tribd.toml` stays read-only ops config; this file is
//! written back by the settings API (atomic temp + rename) and lives
//! beside it — on the boot disk, never under the destination it names.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use trib_project::RecordFormat;

use crate::settings::{SAMPLE_RATES, Settings, SettingsError};

pub const PREFS_SCHEMA_VERSION: u32 = 1;

/// The prefs file's name, resolved as a sibling of the config file so it
/// follows `--config` wherever it points.
const PREFS_FILE: &str = "recording.toml";

pub fn prefs_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name(PREFS_FILE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingPrefs {
    pub schema_version: u32,
    /// Recording destination (a projects root). Absent = tribd.toml's
    /// `projects.root`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub format: RecordFormat,
    /// Takes effect at the next daemon start. Absent = tribd.toml's
    /// `audio.sample_rate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl Default for RecordingPrefs {
    fn default() -> Self {
        RecordingPrefs {
            schema_version: PREFS_SCHEMA_VERSION,
            destination: None,
            format: RecordFormat::default(),
            sample_rate: None,
        }
    }
}

impl RecordingPrefs {
    /// A missing file is the defaults, not an error — a fresh checkout
    /// must boot. A corrupt or invalid file is refused loudly: silently
    /// recording to the wrong place or format is worse than not booting.
    pub fn load(path: &Path) -> Result<Self, SettingsError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RecordingPrefs::default());
            }
            Err(e) => return Err(SettingsError::Invalid(format!("{}: {e}", path.display()))),
        };
        let prefs: RecordingPrefs = toml::from_str(&text)
            .map_err(|e| SettingsError::Invalid(format!("{}: {e}", path.display())))?;
        prefs.validate()?;
        Ok(prefs)
    }

    /// Atomic write: temp file + rename, the manifest pattern.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    pub fn validate(&self) -> Result<(), SettingsError> {
        if let Some(rate) = self.sample_rate
            && !SAMPLE_RATES.contains(&rate)
        {
            return Err(SettingsError::Invalid(format!(
                "sample_rate must be one of {SAMPLE_RATES:?}"
            )));
        }
        if let Some(destination) = &self.destination
            && !destination.is_absolute()
        {
            return Err(SettingsError::Invalid(
                "destination must be an absolute path".into(),
            ));
        }
        Ok(())
    }

    /// The projects root recording should land on.
    pub fn effective_root(&self, settings: &Settings) -> PathBuf {
        self.destination
            .clone()
            .unwrap_or_else(|| settings.projects.root.clone())
    }

    /// The rate the engine should run at (from the next boot on).
    pub fn effective_sample_rate(&self, settings: &Settings) -> u32 {
        self.sample_rate.unwrap_or(settings.audio.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_yields_the_defaults() {
        let prefs = RecordingPrefs::load(Path::new("/nonexistent/recording.toml")).unwrap();
        assert_eq!(prefs.schema_version, PREFS_SCHEMA_VERSION);
        assert_eq!(prefs.destination, None);
        assert_eq!(prefs.format, RecordFormat::Wav32Float);
        assert_eq!(prefs.sample_rate, None);
    }

    #[test]
    fn save_load_round_trips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.toml");
        let prefs = RecordingPrefs {
            schema_version: PREFS_SCHEMA_VERSION,
            destination: Some("/media/user/STAGE".into()),
            format: RecordFormat::Flac24,
            sample_rate: Some(96_000),
        };
        prefs.save(&path).unwrap();
        let loaded = RecordingPrefs::load(&path).unwrap();
        assert_eq!(loaded.destination, prefs.destination);
        assert_eq!(loaded.format, RecordFormat::Flac24);
        assert_eq!(loaded.sample_rate, Some(96_000));
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "the temp file was renamed away"
        );
    }

    #[test]
    fn an_unknown_key_is_refused_not_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.toml");
        std::fs::write(&path, "schema_version = 1\nfromat = \"wav16\"\n").unwrap();
        assert!(
            RecordingPrefs::load(&path).is_err(),
            "typos must not vanish"
        );
    }

    #[test]
    fn a_bad_sample_rate_or_format_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recording.toml");
        std::fs::write(&path, "schema_version = 1\nsample_rate = 12345\n").unwrap();
        assert!(RecordingPrefs::load(&path).is_err());
        std::fs::write(&path, "schema_version = 1\nformat = \"mp3\"\n").unwrap();
        assert!(RecordingPrefs::load(&path).is_err());
    }

    #[test]
    fn a_relative_destination_is_refused() {
        let prefs = RecordingPrefs {
            destination: Some("usb/stick".into()),
            ..RecordingPrefs::default()
        };
        assert!(prefs.validate().is_err());
    }

    #[test]
    fn effective_values_fall_back_to_the_boot_settings() {
        let settings = Settings::default();
        let mut prefs = RecordingPrefs::default();
        assert_eq!(prefs.effective_root(&settings), PathBuf::from("projects"));
        assert_eq!(prefs.effective_sample_rate(&settings), 48_000);
        prefs.destination = Some("/media/x".into());
        prefs.sample_rate = Some(96_000);
        assert_eq!(prefs.effective_root(&settings), PathBuf::from("/media/x"));
        assert_eq!(prefs.effective_sample_rate(&settings), 96_000);
    }

    #[test]
    fn prefs_path_is_a_config_sibling() {
        assert_eq!(
            prefs_path(Path::new("/etc/tribd/tribd.toml")),
            PathBuf::from("/etc/tribd/recording.toml")
        );
    }
}
