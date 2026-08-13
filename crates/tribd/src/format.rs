//! Formatting a USB drive to exFAT, via the appliance's privileged helper.
//!
//! The daemon has no privilege to partition anything — it runs as a
//! nologin service account under a systemd *user* unit. A sudoers drop-in
//! grants exactly one root-owned script, and this module is the seam.
//!
//! The daemon re-validates rather than forwarding. The helper validates
//! independently too; neither layer trusts the other, because each can
//! check something the other cannot — the helper owns kernel facts, and
//! only the daemon knows whether tape is rolling or where it is recording.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Where the appliance image installs the helper. Absent everywhere else,
/// which is exactly how a package install reports the feature unavailable.
pub const HELPER: &str = "/usr/local/lib/tributary/format-drive.sh";

/// mkfs.exfat on a 64 GB stick is seconds; this only has to bound a hang.
const TIMEOUT: Duration = Duration::from_secs(600);

/// exFAT's volume label limit — a filesystem fact, mirrored from the
/// helper's own check so the console can refuse before a round trip. The
/// helper's answer is authoritative; this one only greys out a button.
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

/// A whole-disk device node, and nothing that could be anything else.
/// Partitions are refused here as well as in the helper: formatting one
/// would strand the rest of the drive, which is the problem the feature
/// exists to solve.
pub fn validate_device(device: &str) -> Result<(), String> {
    let ok = device
        .strip_prefix("/dev/")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase()));
    if !ok {
        return Err(format!("not a whole-disk device node: {device}"));
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

/// Blocking: wipe `device` and lay down one exFAT volume labelled `label`.
/// Call via `spawn_blocking`.
pub fn run(device: &str, label: &str) -> Result<(), FormatError> {
    validate_device(device).map_err(FormatError::Refused)?;
    validate_label(label).map_err(FormatError::Refused)?;

    // No shell: args go to the program directly, so the risk being
    // defended against is naming the wrong device, not quoting.
    let out = Command::new("timeout")
        .arg(TIMEOUT.as_secs().to_string())
        .args(["sudo", "-n", HELPER, "format", device, label])
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

    /// A partition is not a format target, and neither is anything that
    /// could walk out of /dev.
    #[test]
    fn only_whole_disk_nodes_are_accepted() {
        assert!(validate_device("/dev/sda").is_ok());
        assert!(validate_device("/dev/sdb").is_ok());
        assert!(validate_device("/dev/sda1").is_err(), "partition");
        assert!(validate_device("/dev/mmcblk0p2").is_err(), "partition");
        assert!(validate_device("/dev/../etc/passwd").is_err());
        assert!(validate_device("sda").is_err());
        assert!(validate_device("/dev/").is_err());
        assert!(validate_device("").is_err());
    }
}
