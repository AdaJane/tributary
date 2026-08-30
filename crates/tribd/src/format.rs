//! Formatting an attached drive, via the appliance's privileged helper.
//!
//! The daemon has no privilege to partition anything — it runs as a
//! nologin service account under a systemd *user* unit. A sudoers drop-in
//! grants exactly one root-owned script, and this module is the seam.
//!
//! The daemon re-validates rather than forwarding. The helper validates
//! independently too; neither layer trusts the other, because each can
//! check something the other cannot — the helper owns kernel facts, and
//! only the daemon knows whether tape is rolling or where it is recording.
//!
//! Note where the line between the two falls. This module guards the
//! *shape* of what it forwards; the helper decides what the device
//! actually is. Deciding "whole disk" from a device name is guesswork —
//! `sda1` is a partition and `nvme0n1` is not, and no amount of string
//! reading settles it — while the helper can read `/sys/dev/block` and the
//! mount table, which settle it exactly.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Where the appliance image installs the helper. Absent everywhere else,
/// which is exactly how a package install reports the feature unavailable.
pub const HELPER: &str = "/usr/local/lib/tributary/format-drive.sh";

/// mkfs.exfat on a 64 GB stick is seconds; this only has to bound a hang.
const TIMEOUT: Duration = Duration::from_secs(600);

/// The filesystem to lay down.
///
/// exFAT for a drive that gets unplugged and opened on a laptop; ext4 for
/// one that lives in the recorder, where journalling and real ownership
/// are worth more than being readable on macOS.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
pub enum Filesystem {
    #[serde(rename = "exfat")]
    Exfat,
    #[serde(rename = "ext4")]
    Ext4,
}

impl Filesystem {
    /// The token the helper takes — and, by the test below, byte-identical
    /// to the one the console sends. Two spellings of one filesystem is
    /// the kind of drift that ends with mkfs never being reached.
    pub const fn as_arg(self) -> &'static str {
        match self {
            Self::Exfat => "exfat",
            Self::Ext4 => "ext4",
        }
    }
}

/// exFAT's volume label limit — a filesystem fact, mirrored from the
/// helper's own check so the console can refuse before a round trip. The
/// helper's answer is authoritative; this one only greys out a button.
///
/// ext4 would allow 16 characters and still gets 11: one rule the helper,
/// the daemon and the console can all state identically is worth more than
/// five characters on one of the two filesystems.
pub fn validate_label(label: &str) -> Result<(), String> {
    if label.is_empty() || label.len() > 11 {
        return Err("label must be 1-11 characters".into());
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err("label may use A-Z a-z 0-9 . _ - only".into());
    }
    Ok(())
}

/// A device node under `/dev`, named by exactly one path component.
///
/// This is a shape guard, not a semantic one. It exists so nothing that
/// could traverse, glob or carry a separator reaches the helper's argument
/// list — and it stops there, deliberately.
///
/// It used to require every character to be lowercase, which read like a
/// whole-disk test and was really an alphabet test: it accepted `sda` and
/// rejected `sda1`, and so also rejected `nvme0n1`, `mmcblk0` and every
/// other disk whose name carries a digit. Whether a node is a whole disk
/// is answered in the helper from `/sys/dev/block/<devno>/partition`,
/// which is the only place that can answer it truthfully.
pub fn validate_device(device: &str) -> Result<(), String> {
    let ok = device.strip_prefix("/dev/").is_some_and(|n| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    });
    if !ok {
        return Err(format!("not a device node: {device}"));
    }
    Ok(())
}

/// Whether this installation can format at all.
///
/// Runs the helper's own `self-check` through sudo instead of testing for
/// the file: presence, the exec bit and the sudoers grant can each be
/// missing independently, and only actually invoking it proves all three.
/// The answer cannot change at runtime, so callers cache it.
pub fn available() -> bool {
    if !Path::new(HELPER).exists() {
        return false;
    }
    Command::new("sudo")
        .args(["-n", HELPER, "self-check"])
        .output()
        .is_ok_and(|out| out.status.success())
}

#[derive(Debug)]
pub enum FormatError {
    /// The helper refused, or validation did. Carries its sentence.
    Refused(String),
    /// Something is using the drive.
    Busy(String),
    Internal(String),
}

/// Blocking: wipe `device` and lay down one volume labelled `label`.
/// Call via `spawn_blocking`.
pub fn run(device: &str, label: &str, filesystem: Filesystem) -> Result<(), FormatError> {
    validate_device(device).map_err(FormatError::Refused)?;
    validate_label(label).map_err(FormatError::Refused)?;

    // No shell: args go to the program directly, so the risk being
    // defended against is naming the wrong device, not quoting.
    let out = Command::new("timeout")
        .arg(TIMEOUT.as_secs().to_string())
        .args([
            "sudo",
            "-n",
            HELPER,
            "format",
            device,
            label,
            filesystem.as_arg(),
        ])
        .output()
        .map_err(|e| FormatError::Internal(e.to_string()))?;

    if out.status.success() {
        return Ok(());
    }
    // The helper's stderr is written for a person; the last line is its
    // verdict, and it goes to the console verbatim.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = stderr
        .lines()
        .rfind(|l| !l.starts_with("stage:"))
        .unwrap_or("format failed")
        .to_owned();
    match out.status.code() {
        Some(2) => Err(FormatError::Refused(detail)),
        Some(3) => Err(FormatError::Busy(detail)),
        _ => Err(FormatError::Internal(detail)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_exfat_limit() {
        assert!(validate_label("TRIBUTARY").is_ok());
        assert!(validate_label("FIELD_REC-1").is_ok());
        assert!(validate_label("").is_err());
        // 12 characters: one past exFAT's limit.
        assert!(validate_label("TRIBUTARYXX2").is_err());
        assert!(validate_label("has space").is_err());
        assert!(validate_label("slash/es").is_err());
        assert!(validate_label("café").is_err());
    }

    /// The shape guard admits any single node name under /dev — including
    /// the ones with digits, which is the whole point — and nothing that
    /// could walk out of it or split into a second argument.
    #[test]
    fn device_names_are_one_component_under_dev() {
        for ok in [
            "/dev/sda",
            "/dev/sdb",
            // The names the old lowercase-only rule silently refused.
            "/dev/nvme0n1",
            "/dev/mmcblk0",
        ] {
            assert!(validate_device(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "/dev/../etc/passwd",
            "/dev/disk/by-id/usb-x",
            "sda",
            "/dev/",
            "",
            "/dev/sda /dev/mmcblk0",
            "/dev/sda;rm -rf /",
            "/dev/sda\u{0}",
        ] {
            assert!(validate_device(bad).is_err(), "{bad:?} should be refused");
        }
    }

    /// Partitions are NOT refused here any more, and that is deliberate:
    /// the name cannot tell you (`nvme0n1` is a disk, `sda1` is not, and
    /// both are "letters then digits"). The helper reads
    /// /sys/dev/block/<devno>/partition and refuses on the evidence.
    #[test]
    fn deciding_whole_disk_is_left_to_the_helper() {
        assert!(validate_device("/dev/sda1").is_ok());
        assert!(validate_device("/dev/nvme0n1p1").is_ok());
    }

    /// One filesystem, one spelling. The wire token, the helper argument
    /// and the enum cannot drift apart without this failing.
    #[test]
    fn the_wire_token_and_the_helper_argument_are_the_same_string() {
        for fs in [Filesystem::Exfat, Filesystem::Ext4] {
            let wire = serde_json::to_string(&fs).expect("serialize");
            assert_eq!(wire, format!("\"{}\"", fs.as_arg()));
            let back: Filesystem = serde_json::from_str(&wire).expect("round trip");
            assert_eq!(back, fs);
        }
    }
}
