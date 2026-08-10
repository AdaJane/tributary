//! The project directory and its manifest. One directory per session
//! ("fresh masking tape"), one TOML document holding the whole mix.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trib_core::MixerState;

pub const SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "project.toml";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub schema_version: u32,
    pub name: String,
    pub created_at_unix: u64,
    pub mixer: MixerState,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub dir: PathBuf,
    pub name: String,
}

impl Project {
    pub fn takes_dir(&self) -> PathBuf {
        self.dir.join("takes")
    }

    /// Next take number: one past the highest existing `take-NNN`.
    pub fn next_take(&self) -> u32 {
        let Ok(entries) = std::fs::read_dir(self.takes_dir()) else {
            return 1;
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_prefix("take-")?
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .map_or(1, |n| n + 1)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after 1970")
        .as_secs()
}

/// A fresh piece of tape: directory named for the moment, manifest seeded
/// with the given console state.
pub fn create_project(
    projects_root: &Path,
    name: &str,
    state: &MixerState,
) -> Result<Project, ProjectError> {
    let created = now_unix();
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let dir = projects_root.join(format!("{created}-{slug}"));
    std::fs::create_dir_all(dir.join("takes"))?;
    let project = Project {
        dir,
        name: name.to_owned(),
    };
    save_manifest(&project, state, created)?;
    Ok(project)
}

/// Atomic write: temp file + rename, so a power cut leaves the previous
/// manifest intact rather than half a document.
pub fn save_manifest(
    project: &Project,
    state: &MixerState,
    created_at_unix: u64,
) -> Result<(), ProjectError> {
    let manifest = ProjectManifest {
        schema_version: SCHEMA_VERSION,
        name: project.name.clone(),
        created_at_unix,
        mixer: state.clone(),
    };
    let text = toml::to_string_pretty(&manifest)?;
    let tmp = project.dir.join(format!("{MANIFEST_FILE}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, project.dir.join(MANIFEST_FILE))?;
    Ok(())
}

pub fn load_manifest(dir: &Path) -> Result<ProjectManifest, ProjectError> {
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
    Ok(toml::from_str(&text)?)
}

#[derive(Debug, Clone, Deserialize)]
pub struct TakeTrackInfo {
    pub file: String,
    pub channels: u16,
    pub frames: u64,
    pub dropped_samples: u64,
    /// Absent in takes cut before the field existed — the `chNN-` filename
    /// prefix is the fallback association.
    #[serde(default)]
    pub strip_id: Option<u32>,
}

/// One recorded take, read back from its `take.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct TakeInfo {
    #[serde(default)]
    pub schema_version: u32,
    pub started_at_unix: u64,
    pub sample_rate: u32,
    /// Absent in takes cut before formats were configurable — those are
    /// 32-bit-float WAV by construction.
    #[serde(default)]
    pub format: crate::format::RecordFormat,
    pub damaged: bool,
    pub tracks: Vec<TakeTrackInfo>,
    /// Not in the manifest — derived from the directory name.
    #[serde(skip)]
    pub take: u32,
}

/// All finished takes, newest first. A take without a readable manifest
/// (mid-recording, or torn) is skipped — the writer owns that file.
pub fn list_takes(project: &Project) -> Vec<TakeInfo> {
    let Ok(entries) = std::fs::read_dir(project.takes_dir()) else {
        return Vec::new();
    };
    let mut takes: Vec<TakeInfo> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let number: u32 = entry
                .file_name()
                .to_str()?
                .strip_prefix("take-")?
                .parse()
                .ok()?;
            let text = std::fs::read_to_string(entry.path().join("take.toml")).ok()?;
            let mut info: TakeInfo = toml::from_str(&text).ok()?;
            info.take = number;
            Some(info)
        })
        .collect();
    takes.sort_by_key(|info| std::cmp::Reverse(info.take));
    takes
}

/// The most recently created project under the root, if any. Unreadable
/// manifests are skipped with a warning — one corrupt directory must not
/// stop the daemon from booting.
pub fn load_latest(projects_root: &Path) -> Option<(Project, ProjectManifest)> {
    let entries = std::fs::read_dir(projects_root).ok()?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs.into_iter().rev() {
        match load_manifest(&dir) {
            Ok(manifest) => {
                let project = Project {
                    dir,
                    name: manifest.name.clone(),
                };
                return Some((project, manifest));
            }
            Err(e) => tracing::warn!(dir = %dir.display(), %e, "skipping unreadable project"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use trib_core::{StripId, StripState};

    use super::*;

    fn state() -> MixerState {
        MixerState {
            strips: vec![StripState::new(StripId(0), "Vocal".into())],
            ..MixerState::default()
        }
    }

    #[test]
    fn create_save_load_round_trips_the_mixer() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "Friday Night", &state()).unwrap();
        let (loaded, manifest) = load_latest(root.path()).unwrap();
        assert_eq!(loaded.name, "Friday Night");
        assert_eq!(manifest.schema_version, SCHEMA_VERSION);
        assert_eq!(manifest.mixer, state());
        assert_eq!(project.next_take(), 1);
    }

    #[test]
    fn next_take_counts_past_existing_dirs() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "x", &state()).unwrap();
        std::fs::create_dir_all(project.takes_dir().join("take-001")).unwrap();
        std::fs::create_dir_all(project.takes_dir().join("take-007")).unwrap();
        assert_eq!(project.next_take(), 8, "gaps are not refilled");
    }

    #[test]
    fn list_takes_reads_manifests_newest_first_and_skips_torn_ones() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "x", &state()).unwrap();
        for (n, damaged) in [(1, false), (3, true)] {
            let dir = project.takes_dir().join(format!("take-{n:03}"));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("take.toml"),
                format!(
                    "schema_version = 1\nstarted_at_unix = 100\nsample_rate = 48000\n\
                     damaged = {damaged}\n\n[[tracks]]\nfile = \"master.wav\"\n\
                     channels = 2\nframes = 96000\ndropped_samples = 0\n"
                ),
            )
            .unwrap();
        }
        // Mid-recording: a take dir with no manifest yet.
        std::fs::create_dir_all(project.takes_dir().join("take-004")).unwrap();

        let takes = list_takes(&project);
        assert_eq!(takes.len(), 2, "the manifest-less take is invisible");
        assert_eq!(takes[0].take, 3, "newest first");
        assert!(takes[0].damaged);
        assert_eq!(takes[1].tracks[0].frames, 96_000);
    }

    #[test]
    fn a_pre_multi_device_manifest_patch_loads_as_the_default_device() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "old", &state()).unwrap();
        // The exact on-disk shape every pre-multi-device autosave wrote.
        let manifest_text = std::fs::read_to_string(project.dir.join("project.toml")).unwrap();
        let patched = manifest_text.replace(
            "[[mixer.strips]]",
            "[[mixer.strips]]\n# device-less input appended below",
        );
        let with_input = format!("{patched}\n[mixer.strips.input]\ndevice_channel = 3\n");
        std::fs::write(project.dir.join("project.toml"), with_input).unwrap();

        let (_, manifest) = load_latest(root.path()).unwrap();
        let input = manifest.mixer.strips[0].input.as_ref().unwrap();
        assert_eq!(input.device, None, "absent field = system default input");
        assert_eq!(input.device_channel, 3);
    }

    #[test]
    fn a_named_device_patch_round_trips_through_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let mut mixer = state();
        mixer.strips[0].input = Some(trib_core::InputAssign {
            device: Some("ThinkPad Thunderbolt 4 Dock USB".into()),
            device_channel: 1,
        });
        create_project(root.path(), "multi", &mixer).unwrap();
        let (_, manifest) = load_latest(root.path()).unwrap();
        assert_eq!(manifest.mixer, mixer);
    }

    #[test]
    fn a_corrupt_manifest_is_skipped_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        create_project(root.path(), "good", &state()).unwrap();
        let bad = root.path().join("zzz-newest-but-broken");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("project.toml"), "not toml [ at all").unwrap();
        let (project, _) = load_latest(root.path()).unwrap();
        assert_eq!(project.name, "good");
    }
}
