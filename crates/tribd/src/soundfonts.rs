//! The SoundFont library: what is on this box, and whether it can be used.
//!
//! Two sources, deliberately kept apart in the report. The internal root is
//! the box's own store — uploads land there and it is always present. A
//! removable volume is somebody's stick, so its files are listed but never
//! deleted from here, and they vanish when it is unplugged.
//!
//! Everything here is blocking filesystem work, so it runs on the
//! instrument host thread or under `spawn_blocking` — never on the control
//! task, which owes the console a fader that keeps moving.

use std::path::{Path, PathBuf};

/// The one extension the loader will take. SF3 (Ogg-compressed) parses as a
/// SoundFont but `rustysynth` refuses its samples, so accepting the name
/// would trade a clear "not a soundfont" for a confusing "loaded, silent".
pub const SOUNDFONT_EXT: &str = "sf2";

/// Depth limit for a removable volume: the volume root and one
/// `soundfonts/` directory under it. Walking a mounted terabyte drive
/// looking for `.sf2` is not a feature, it is a hang.
const USB_SUBDIR: &str = "soundfonts";

/// Where a file came from. Drives what the console offers to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoundfontOrigin {
    /// The box's own library. Uploadable, deletable.
    Internal,
    /// A mounted volume. Listed and playable, never deleted from here.
    Removable,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct SoundfontInfo {
    /// Library id — the file name, which is also what an instrument stores.
    pub id: String,
    pub path: String,
    pub bytes: u64,
    pub origin: SoundfontOrigin,
    /// Volume label for a removable file, so the console can say *which*
    /// stick it wants back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
}

/// Does this name look like a SoundFont we would try to load?
///
/// Case-insensitive: `.SF2` off a Windows-formatted stick is the same file.
pub fn looks_like_soundfont(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(SOUNDFONT_EXT))
}

/// The RIFF header every SoundFont starts with: `RIFF`, four size bytes,
/// then `sfbk`.
///
/// Checked before a single byte of an upload is written, because the
/// alternative is filling the disk with 300 MB of somebody's holiday video
/// and only then saying no.
pub fn has_soundfont_header(head: &[u8]) -> bool {
    head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"sfbk"
}

/// A file name that cannot escape the library.
///
/// The client's file name is never trusted: it is reduced to one path
/// component of safe characters with the right extension, or refused. This
/// is the `project::resolve` doctrine applied to the other direction — a
/// name that traverses is not a name.
pub fn safe_file_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let stem = Path::new(raw).file_stem()?.to_str()?;
    if !looks_like_soundfont(raw) {
        return None;
    }
    let slug: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim().trim_matches('.').trim().to_owned();
    if slug.is_empty() {
        return None;
    }
    Some(format!("{slug}.{SOUNDFONT_EXT}"))
}

fn entries_in(dir: &Path, origin: SoundfontOrigin, volume: Option<&str>) -> Vec<SoundfontInfo> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SoundfontInfo> = read
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?.to_owned();
            if !looks_like_soundfont(&name) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some(SoundfontInfo {
                id: name,
                path: path.to_string_lossy().into_owned(),
                bytes: meta.len(),
                origin,
                volume: volume.map(str::to_owned),
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Everything the box can currently load, internal first.
///
/// `mounts` are the removable volumes to sweep — the caller passes what the
/// destinations enumeration already knows, so there is one place that
/// decides what "a drive" is.
pub fn list(root: &Path, mounts: &[(PathBuf, String)]) -> Vec<SoundfontInfo> {
    let mut out = entries_in(root, SoundfontOrigin::Internal, None);
    for (mount, label) in mounts {
        // The volume root, then its `soundfonts/` directory. Two shallow
        // reads, never a walk.
        out.extend(entries_in(mount, SoundfontOrigin::Removable, Some(label)));
        out.extend(entries_in(
            &mount.join(USB_SUBDIR),
            SoundfontOrigin::Removable,
            Some(label),
        ));
    }
    out
}

/// Resolve a library id to a readable path.
///
/// An id is a bare file name by construction, so a traversal cannot be
/// expressed — but it is re-validated here rather than trusted, because
/// this id arrives from a manifest that may have been hand-edited.
pub fn resolve(root: &Path, mounts: &[(PathBuf, String)], id: &str) -> Option<PathBuf> {
    if safe_file_name(id).as_deref() != Some(id) {
        return None;
    }
    let internal = root.join(id);
    if internal.is_file() {
        return Some(internal);
    }
    mounts.iter().find_map(|(mount, _)| {
        [mount.join(id), mount.join(USB_SUBDIR).join(id)]
            .into_iter()
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sf2_names_look_like_soundfonts() {
        assert!(looks_like_soundfont("piano.sf2"));
        // A stick formatted on Windows is the same file.
        assert!(looks_like_soundfont("PIANO.SF2"));
        assert!(!looks_like_soundfont("piano.sf3"));
        assert!(!looks_like_soundfont("piano"));
        assert!(!looks_like_soundfont("piano.sf2.txt"));
    }

    #[test]
    fn the_header_check_reads_the_riff_form_not_just_the_magic() {
        assert!(has_soundfont_header(b"RIFF\x00\x00\x00\x00sfbk...."));
        // A WAV is also RIFF — accepting it would turn "not a soundfont"
        // into a silent instrument nobody can explain.
        assert!(!has_soundfont_header(b"RIFF\x00\x00\x00\x00WAVEfmt "));
        assert!(!has_soundfont_header(b"RIFF"));
    }

    #[test]
    fn an_uploaded_name_is_slugged_and_can_never_escape_the_library() {
        assert_eq!(safe_file_name("piano.sf2").as_deref(), Some("piano.sf2"));
        assert_eq!(
            safe_file_name("Grand Piano.SF2").as_deref(),
            Some("Grand Piano.sf2")
        );
        assert_eq!(
            safe_file_name("../../etc/passwd.sf2").as_deref(),
            Some("passwd.sf2")
        );
        assert_eq!(
            safe_file_name("/etc/shadow.sf2").as_deref(),
            Some("shadow.sf2")
        );
        assert_eq!(safe_file_name("notes.txt"), None);
        assert_eq!(safe_file_name("...sf2"), None);
    }

    #[test]
    fn the_library_lists_internal_files_before_removable_ones() {
        let root = tempfile::tempdir().unwrap();
        let stick = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("internal.sf2"), b"x").unwrap();
        std::fs::write(stick.path().join("stick.sf2"), b"xx").unwrap();
        std::fs::create_dir(stick.path().join("soundfonts")).unwrap();
        std::fs::write(stick.path().join("soundfonts/nested.sf2"), b"xxx").unwrap();
        std::fs::write(stick.path().join("notes.txt"), b"ignored").unwrap();

        let mounts = vec![(stick.path().to_path_buf(), "STICK".to_owned())];
        let found = list(root.path(), &mounts);

        let ids: Vec<&str> = found.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["internal.sf2", "stick.sf2", "nested.sf2"]);
        assert_eq!(found[0].origin, SoundfontOrigin::Internal);
        assert_eq!(found[1].origin, SoundfontOrigin::Removable);
        assert_eq!(found[1].volume.as_deref(), Some("STICK"));
        assert_eq!(found[2].bytes, 3);
    }

    #[test]
    fn a_missing_root_is_an_empty_library_not_a_failure() {
        // A fresh install has no soundfonts directory until the first
        // upload; that must read as "none yet", never as a boot error.
        let found = list(Path::new("/definitely/not/here"), &[]);
        assert!(found.is_empty());
    }

    #[test]
    fn resolve_finds_a_file_on_a_stick_and_refuses_a_traversing_id() {
        let root = tempfile::tempdir().unwrap();
        let stick = tempfile::tempdir().unwrap();
        std::fs::create_dir(stick.path().join("soundfonts")).unwrap();
        std::fs::write(stick.path().join("soundfonts/pad.sf2"), b"x").unwrap();
        let mounts = vec![(stick.path().to_path_buf(), "STICK".to_owned())];

        assert_eq!(
            resolve(root.path(), &mounts, "pad.sf2"),
            Some(stick.path().join("soundfonts/pad.sf2"))
        );
        assert_eq!(resolve(root.path(), &mounts, "../pad.sf2"), None);
        assert_eq!(resolve(root.path(), &mounts, "missing.sf2"), None);
    }
}
