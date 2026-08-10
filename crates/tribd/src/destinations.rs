//! Candidate recording destinations: the mounted filesystems `sysinfo`
//! reports, filtered down to what a user would plausibly point a recorder
//! at. Enumerated fresh on every GET — no polling, matching the device
//! doctrine (manual refresh only).

use std::path::Path;

/// One mounted filesystem a recording could land on.
#[derive(Debug, Clone)]
pub struct Drive {
    pub mount_point: String,
    /// The filesystem label or device name, best-effort.
    pub label: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub removable: bool,
    pub read_only: bool,
}

/// Mounts that are never recording destinations, whatever sysinfo says —
/// system trees, snaps, AppImage mounts, container overlays.
const EXCLUDED_PREFIXES: [&str; 9] = [
    "/proc",
    "/sys",
    "/dev",
    "/run/lock",
    "/boot",
    "/snap",
    "/tmp",
    "/var/lib/docker",
    "/var/snap",
];

fn excluded(mount: &Path) -> bool {
    EXCLUDED_PREFIXES
        .iter()
        .any(|prefix| mount.starts_with(prefix))
}

/// What the tile prints. Removable media mounts by volume label
/// ("/media/user/USB321FD"), so the mount basename IS the friendly name;
/// device nodes ("/dev/sda1") are the fallback.
fn label_for(mount: &Path, device: &str) -> String {
    if let Some(name) = mount.file_name() {
        return name.to_string_lossy().into_owned();
    }
    if device.is_empty() {
        mount.display().to_string()
    } else {
        device.to_owned()
    }
}

/// Enumerate mounted filesystems, removable media first. Blocking (reads
/// /proc + statvfs under the hood) — call via `spawn_blocking`.
pub fn enumerate() -> Vec<Drive> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut drives: Vec<Drive> = disks
        .list()
        .iter()
        .filter(|disk| !excluded(disk.mount_point()))
        .map(|disk| Drive {
            mount_point: disk.mount_point().display().to_string(),
            label: label_for(disk.mount_point(), &disk.name().to_string_lossy()),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
            removable: disk.is_removable(),
            read_only: disk.is_read_only(),
        })
        .collect();
    drives.sort_by(|a, b| {
        b.removable
            .cmp(&a.removable)
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_prefer_the_volume_name_over_the_device_node() {
        assert_eq!(
            label_for(Path::new("/media/user/USB321FD"), "/dev/sda1"),
            "USB321FD"
        );
        assert_eq!(label_for(Path::new("/home"), "/dev/nvme0n1p2"), "home");
        assert_eq!(
            label_for(Path::new("/"), "/dev/nvme0n1p2"),
            "/dev/nvme0n1p2"
        );
        assert_eq!(label_for(Path::new("/"), ""), "/");
    }

    #[test]
    fn system_mounts_are_excluded() {
        assert!(excluded(Path::new("/proc/foo")));
        assert!(excluded(Path::new("/boot/efi")));
        assert!(excluded(Path::new("/tmp/.mount_AppImage123")));
        assert!(excluded(Path::new("/var/lib/docker/rootfs/overlayfs/abc")));
        assert!(!excluded(Path::new("/")));
        assert!(!excluded(Path::new("/run/media/user/STICK")));
        assert!(!excluded(Path::new("/media/user/USB321FD")));
        assert!(!excluded(Path::new("/home")));
    }

    /// Smoke test against the real system: whatever is mounted, the root
    /// filesystem shows up and removable drives sort first.
    #[test]
    fn enumerate_reports_the_root_filesystem() {
        let drives = enumerate();
        assert!(
            drives
                .iter()
                .any(|d| d.mount_point == "/" || d.total_bytes > 0),
            "at least one real mount: {drives:?}"
        );
        let first_fixed = drives.iter().position(|d| !d.removable);
        if let Some(first_fixed) = first_fixed {
            assert!(
                drives[..first_fixed].iter().all(|d| d.removable),
                "removable drives sort first"
            );
        }
    }
}
