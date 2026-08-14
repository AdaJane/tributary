//! The project directory and its manifest. One directory per session
//! ("fresh masking tape"), one TOML document holding the whole mix.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trib_core::MixerState;

pub const SCHEMA_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "project.toml";

/// Longest session name we accept. The name is slugged into a directory
/// component, and 255 bytes is the ceiling on both ext4 and exFAT — this
/// leaves room for the `{unix}-` prefix with a wide margin.
const MAX_NAME: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("manifest: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("name: {0}")]
    BadName(&'static str),
    /// A client-supplied id that is not a single, ordinary path component.
    #[error("not a session id")]
    BadId,
    /// The id resolved outside the root, or to a directory holding no
    /// manifest — so not a session this daemon created.
    #[error("no session there")]
    NotASession,
    /// A session directory of that name already exists.
    #[error("a session of that name already exists")]
    Exists,
}

/// Trimmed, single-line, 1..=64 characters.
///
/// Traversal through the *name* is already impossible — `create_project`
/// slugs every non-alphanumeric character to `-`, so `"../../etc"` becomes
/// `"-------"`. This guards length and legibility, not containment; that
/// job belongs to [`resolve`].
pub fn validate_name(name: &str) -> Result<&str, ProjectError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ProjectError::BadName("name the session"));
    }
    if trimmed.chars().count() > MAX_NAME {
        return Err(ProjectError::BadName("at most 64 characters"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(ProjectError::BadName("one line, no control characters"));
    }
    Ok(trimmed)
}

/// A directory proved to be a session directly under a projects root.
///
/// Constructed only by [`resolve`], so nothing downstream can be handed a
/// client's path by accident — the type *is* the proof.
#[derive(Debug, Clone)]
pub struct SessionDir(PathBuf);

impl SessionDir {
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The directory name — the stable id a client holds.
    pub fn id(&self) -> &str {
        self.0
            .file_name()
            .and_then(|n| n.to_str())
            .expect("resolve only builds from a validated component")
    }
}

/// Turn a client-supplied id into a session directory, or refuse.
///
/// The one place a client string becomes a path, and the security boundary
/// of the whole sessions feature. Three gates, each covering something the
/// others do not:
///
/// 1. The id must be exactly one ordinary path component that round-trips
///    to the string we were given. Required because axum percent-decodes
///    path params *before* the handler sees them, so `..%2F..%2Fetc`
///    arrives here as the single string `../../etc`.
/// 2. Canonicalised, the candidate must be a DIRECT CHILD of the
///    canonicalised root. `starts_with` would wave through
///    `<root>/gig/takes/take-001`; canonicalising is what defeats a
///    symlink planted in the root.
/// 3. It must hold a manifest. The destination is user-chosen and can
///    legitimately be a home directory — without this, listing and
///    deleting sessions would be a file manager for it.
pub fn resolve(projects_root: &Path, id: &str) -> Result<SessionDir, ProjectError> {
    let mut parts = Path::new(id).components();
    match (parts.next(), parts.next()) {
        (Some(std::path::Component::Normal(part)), None) if part == id => {}
        _ => return Err(ProjectError::BadId),
    }
    let root = projects_root
        .canonicalize()
        .map_err(|_| ProjectError::NotASession)?;
    let dir = root
        .join(id)
        .canonicalize()
        .map_err(|_| ProjectError::NotASession)?;
    if dir.parent() != Some(root.as_path()) {
        return Err(ProjectError::NotASession);
    }
    if !dir.join(MANIFEST_FILE).is_file() {
        return Err(ProjectError::NotASession);
    }
    Ok(SessionDir(dir))
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
    let name = validate_name(name)?;
    let created = now_unix();
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let dir = projects_root.join(format!("{created}-{slug}"));
    // `create_dir`, deliberately not `create_dir_all`: the latter succeeds
    // on an existing directory, so two creates in the same second would
    // silently land on one — the second overwriting the first's manifest
    // and inheriting its takes. The failing mkdir IS the collision check.
    std::fs::create_dir_all(projects_root)?;
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ProjectError::Exists);
        }
        Err(e) => return Err(e.into()),
    }
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
    /// MIDI sidecars, one per instrument that was armed. Absent in every
    /// take cut before instruments existed.
    #[serde(default)]
    pub midi_tracks: Vec<TakeMidiTrackInfo>,
    /// Not in the manifest — derived from the directory name.
    #[serde(skip)]
    pub take: u32,
}

/// One MIDI sidecar of a take.
#[derive(Debug, Clone, Deserialize)]
pub struct TakeMidiTrackInfo {
    pub file: String,
    pub name: String,
    pub events: u64,
    #[serde(default)]
    pub dropped_events: u64,
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

/// Candidate project directories, newest first.
///
/// The directory name leads with the creation timestamp, so a lexicographic
/// sort is chronological. `load_latest` and `list_sessions` share this so
/// they can never disagree about which session is newest — the head of the
/// browser's list must be the one an adopt would open.
fn project_dirs(projects_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(projects_root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.reverse();
    dirs
}

/// One session, as the browser lists it. Deliberately not the manifest:
/// a list must not carry a full `MixerState` per row.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// The directory name — the stable id, unchanged by a rename.
    pub id: String,
    pub name: String,
    pub created_at_unix: u64,
    pub take_count: usize,
}

/// Every readable session under the root, newest first.
///
/// Take counting stats `take.toml` rather than parsing it: thirty sessions
/// of forty takes would otherwise be twelve hundred TOML parses per list,
/// over USB. The one divergence from [`list_takes`] is that a present but
/// corrupt manifest is counted here and hidden there.
///
/// Blocking. Degrades like `load_latest` rather than failing — an
/// unreadable root is an empty list, which is what a drive with nothing on
/// it should render as.
pub fn list_sessions(projects_root: &Path) -> Vec<SessionSummary> {
    project_dirs(projects_root)
        .into_iter()
        .filter_map(|dir| {
            let manifest = load_manifest(&dir).ok()?;
            let id = dir.file_name()?.to_str()?.to_owned();
            Some(SessionSummary {
                id,
                name: manifest.name,
                created_at_unix: manifest.created_at_unix,
                take_count: count_takes(&dir),
            })
        })
        .collect()
}

fn count_takes(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir.join("takes")) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("take.toml").is_file())
        .count()
}

/// Open one specific session — the by-id counterpart to `load_latest`,
/// which is otherwise the only way a project ever gets opened.
pub fn open_session(dir: &SessionDir) -> Result<(Project, ProjectManifest), ProjectError> {
    let manifest = load_manifest(dir.path())?;
    Ok((
        Project {
            dir: dir.path().to_owned(),
            name: manifest.name.clone(),
        },
        manifest,
    ))
}

/// The manifest of a project we already hold — for a freshly created one,
/// where there is no id to resolve through yet.
pub fn manifest_of(project: &Project) -> Result<ProjectManifest, ProjectError> {
    load_manifest(&project.dir)
}

/// Retitle a session: read the manifest, replace the name, write it back.
///
/// The DIRECTORY is never renamed, and that is deliberate. Its name is the
/// session's identity — the id in every client's list and in any in-flight
/// request — and `load_latest` orders sessions by it, so re-slugging would
/// silently move a session in the boot-time "newest" ordering. It also
/// keeps open take paths valid: the take writer captures its directory up
/// front and the playback feeder reopens files on every loop pass.
pub fn rename_session(dir: &SessionDir, new_name: &str) -> Result<ProjectManifest, ProjectError> {
    let new_name = validate_name(new_name)?;
    let manifest = load_manifest(dir.path())?;
    let project = Project {
        dir: dir.path().to_owned(),
        name: new_name.to_owned(),
    };
    save_manifest(&project, &manifest.mixer, manifest.created_at_unix)?;
    load_manifest(dir.path())
}

/// Permanently remove a session and every take in it. The first
/// destructive filesystem operation in this crate.
///
/// The manifest is unlinked first so the delete commits at a single
/// `unlink`: `remove_dir_all` is not atomic, and a power cut mid-delete on
/// a USB stick is a realistic event here. After that first step any
/// remainder is inert — both `list_sessions` and `load_latest` require a
/// readable manifest, so it is never listed, never adopted and never
/// recorded into. It does still occupy space until someone clears it.
///
/// Takes the proof token by value: deleting twice is unrepresentable.
pub fn delete_session(dir: SessionDir) -> Result<(), ProjectError> {
    std::fs::remove_file(dir.path().join(MANIFEST_FILE))?;
    std::fs::remove_dir_all(dir.path())?;
    Ok(())
}

/// Remove one take — audio, peaks sidecars and manifest together.
///
/// The number is formatted into `take-NNN` here, so no caller-supplied
/// path ever reaches the filesystem. Note `Project::next_take` is max + 1,
/// so deleting the newest take frees its number for the next recording;
/// deleting a middle one leaves a permanent gap. Both are intended.
pub fn delete_take(project: &Project, take: u32) -> Result<(), ProjectError> {
    std::fs::remove_dir_all(project.takes_dir().join(format!("take-{take:03}")))?;
    Ok(())
}

/// The most recently created project under the root, if any. Unreadable
/// manifests are skipped with a warning — one corrupt directory must not
/// stop the daemon from booting.
pub fn load_latest(projects_root: &Path) -> Option<(Project, ProjectManifest)> {
    for dir in project_dirs(projects_root) {
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

    /// A minimal finished take, for tests that only care that one exists.
    const TAKE: &str = "schema_version = 1\nstarted_at_unix = 100\nsample_rate = 48000\n\
                        damaged = false\n\n[[tracks]]\nfile = \"master.wav\"\n\
                        channels = 2\nframes = 96000\ndropped_samples = 0\n";

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

    /// The security test. Every one of these strings is something a client
    /// can put in a URL path segment, and `resolve` is the only thing
    /// between them and `remove_dir_all`.
    #[test]
    fn resolve_refuses_anything_that_is_not_a_session_directly_under_the_root() {
        let root = tempfile::tempdir().unwrap();
        create_project(root.path(), "gig", &state()).unwrap();
        let id = list_sessions(root.path())[0].id.clone();
        // The happy path, so the refusals below mean something.
        assert!(resolve(root.path(), &id).is_ok());

        for bad in [
            "",
            ".",
            "..",
            "../..",
            // What `..%2F..%2Fetc` becomes: axum percent-decodes before the
            // handler sees it, so a scan for '/' in the raw param is not
            // enough — the component check is what catches this.
            "../../etc",
            "a/b",
            "/etc",
            "/",
            "gig/takes",
        ] {
            assert!(
                matches!(
                    resolve(root.path(), bad),
                    Err(ProjectError::BadId | ProjectError::NotASession)
                ),
                "resolve accepted {bad:?}"
            );
        }

        // A real directory under the root that this daemon did not create.
        // The destination is user-chosen and can be a home directory, so
        // without the manifest check this API would be a file manager.
        std::fs::create_dir(root.path().join("holiday-photos")).unwrap();
        assert!(matches!(
            resolve(root.path(), "holiday-photos"),
            Err(ProjectError::NotASession)
        ));

        // A nested directory of a real session is not itself a session.
        assert!(resolve(root.path(), &format!("{id}/takes")).is_err());
    }

    /// Two sessions created inside one second must not merge. Before the
    /// `create_dir` fix, the second silently overwrote the first's manifest
    /// and inherited its takes.
    #[test]
    fn two_sessions_created_in_the_same_second_do_not_collide() {
        let root = tempfile::tempdir().unwrap();
        let first = create_project(root.path(), "gig", &state()).unwrap();
        std::fs::create_dir_all(first.dir.join("takes/take-001")).unwrap();
        std::fs::write(first.dir.join("takes/take-001/take.toml"), TAKE).unwrap();

        // Same name, same second: refused rather than silently merged.
        assert!(matches!(
            create_project(root.path(), "gig", &state()),
            Err(ProjectError::Exists)
        ));
        // And the first session is untouched.
        let sessions = list_sessions(root.path());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "gig");
        assert_eq!(sessions[0].take_count, 1);
    }

    #[test]
    fn a_name_that_is_empty_or_too_long_or_multiline_is_refused() {
        assert!(validate_name("Friday Night").is_ok());
        // Trimmed, not rejected.
        assert_eq!(validate_name("  Gig  ").unwrap(), "Gig");
        for bad in ["", "   ", "line\nbreak", "nul\0byte"] {
            assert!(validate_name(bad).is_err(), "accepted {bad:?}");
        }
        assert!(validate_name(&"x".repeat(64)).is_ok());
        // 65 characters would push the slugged directory component toward
        // the 255-byte filesystem ceiling.
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn list_sessions_reports_every_session_newest_first_with_take_counts() {
        let root = tempfile::tempdir().unwrap();
        // Distinct directory names, since the prefix is a whole second.
        for (dir, name) in [("100-old", "old"), ("200-mid", "mid"), ("300-new", "new")] {
            let project = Project {
                dir: root.path().join(dir),
                name: name.into(),
            };
            std::fs::create_dir_all(project.dir.join("takes")).unwrap();
            save_manifest(&project, &state(), 1).unwrap();
        }
        let with_takes = root.path().join("200-mid/takes");
        for take in ["take-001", "take-002"] {
            std::fs::create_dir_all(with_takes.join(take)).unwrap();
            std::fs::write(with_takes.join(take).join("take.toml"), TAKE).unwrap();
        }
        // A take directory with no manifest is mid-recording or torn, and
        // is invisible to `list_takes` — so it must not be counted here.
        std::fs::create_dir_all(with_takes.join("take-003")).unwrap();

        let sessions = list_sessions(root.path());
        assert_eq!(
            sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["new", "mid", "old"]
        );
        assert_eq!(sessions[1].id, "200-mid");
        assert_eq!(sessions[1].take_count, 2);
        assert_eq!(sessions[0].take_count, 0);
    }

    /// The head of the browser's list must be the session an adopt opens,
    /// or the console and the daemon disagree about what is current.
    #[test]
    fn list_sessions_and_load_latest_agree_on_the_newest() {
        let root = tempfile::tempdir().unwrap();
        for (dir, name) in [("100-old", "old"), ("300-new", "new")] {
            let project = Project {
                dir: root.path().join(dir),
                name: name.into(),
            };
            std::fs::create_dir_all(project.dir.join("takes")).unwrap();
            save_manifest(&project, &state(), 1).unwrap();
        }
        // A directory with no manifest must not win either race.
        std::fs::create_dir_all(root.path().join("999-not-a-session")).unwrap();

        let listed = &list_sessions(root.path())[0];
        let (adopted, _) = load_latest(root.path()).unwrap();
        assert_eq!(listed.name, adopted.name);
        assert_eq!(
            listed.id,
            adopted.dir.file_name().unwrap().to_str().unwrap()
        );
    }

    #[test]
    fn a_rename_moves_the_manifest_name_and_never_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let created = create_project(root.path(), "gig", &state()).unwrap();
        let id = created
            .dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        let dir = resolve(root.path(), &id).unwrap();
        rename_session(&dir, "Saturday").unwrap();

        // The directory — the identity — is untouched, so outstanding ids
        // stay valid and the boot-time ordering does not move.
        assert!(created.dir.is_dir());
        assert_eq!(list_sessions(root.path())[0].id, id);
        assert_eq!(list_sessions(root.path())[0].name, "Saturday");
        assert!(resolve(root.path(), &id).is_ok());
        // And the console survived the round trip.
        let (_, manifest) = open_session(&resolve(root.path(), &id).unwrap()).unwrap();
        assert_eq!(manifest.mixer, state());
    }

    #[test]
    fn delete_removes_the_session_and_leaves_its_siblings_alone() {
        let root = tempfile::tempdir().unwrap();
        for (dir, name) in [("100-keep", "keep"), ("200-drop", "drop")] {
            let project = Project {
                dir: root.path().join(dir),
                name: name.into(),
            };
            std::fs::create_dir_all(project.dir.join("takes/take-001")).unwrap();
            std::fs::write(project.dir.join("takes/take-001/take.toml"), TAKE).unwrap();
            save_manifest(&project, &state(), 1).unwrap();
        }
        delete_session(resolve(root.path(), "200-drop").unwrap()).unwrap();

        assert!(!root.path().join("200-drop").exists());
        let left = list_sessions(root.path());
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "keep");
        assert_eq!(left[0].take_count, 1);
    }

    /// The interrupted-delete invariant. The real interruption is not
    /// unit-testable without fault injection, so this pins the property it
    /// relies on: once the manifest is gone the remainder is inert.
    #[test]
    fn a_session_whose_manifest_is_gone_is_invisible_to_list_and_load_latest() {
        let root = tempfile::tempdir().unwrap();
        let good = Project {
            dir: root.path().join("100-good"),
            name: "good".into(),
        };
        std::fs::create_dir_all(good.dir.join("takes")).unwrap();
        save_manifest(&good, &state(), 1).unwrap();

        let torn = root.path().join("900-half-deleted");
        std::fs::create_dir_all(torn.join("takes/take-001")).unwrap();

        assert_eq!(list_sessions(root.path()).len(), 1);
        assert_eq!(load_latest(root.path()).unwrap().0.name, "good");
        assert!(matches!(
            resolve(root.path(), "900-half-deleted"),
            Err(ProjectError::NotASession)
        ));
    }

    #[test]
    fn delete_take_removes_the_whole_take_and_frees_the_newest_number() {
        let root = tempfile::tempdir().unwrap();
        let project = create_project(root.path(), "gig", &state()).unwrap();
        for take in ["take-001", "take-002", "take-003"] {
            let dir = project.takes_dir().join(take);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("take.toml"), TAKE).unwrap();
            std::fs::write(dir.join("ch01-test.wav"), b"audio").unwrap();
            std::fs::write(dir.join("ch01-test.peaks"), b"peaks").unwrap();
        }
        assert_eq!(project.next_take(), 4);

        delete_take(&project, 3).unwrap();
        assert!(!project.takes_dir().join("take-003").exists());
        assert_eq!(list_takes(&project).len(), 2);
        // Deleting the newest frees its number — rewind and record over it.
        assert_eq!(project.next_take(), 3);

        // Deleting a middle take leaves a permanent gap, which is correct.
        delete_take(&project, 1).unwrap();
        assert_eq!(project.next_take(), 3);
        assert_eq!(list_takes(&project).len(), 1);
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
        assert_eq!(
            input.device_name(),
            Some(None),
            "absent field = system default input"
        );
        assert_eq!(input.channel(), 3);
    }

    #[test]
    fn a_named_device_patch_round_trips_through_the_manifest() {
        let root = tempfile::tempdir().unwrap();
        let mut mixer = state();
        mixer.strips[0].input = Some(trib_core::InputAssign::device(
            Some("ThinkPad Thunderbolt 4 Dock USB".into()),
            1,
        ));
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
