//! Candidate recording destinations.
//!
//! The spine is `lsblk`, not the mount table. A drive that is plugged in
//! but unmounted is exactly the case the console has to be able to talk
//! about — reporting only what `sysinfo` sees renders a present drive as
//! nothing at all, indistinguishable from an empty port. `sysinfo` stays
//! for the one thing the block layer cannot answer: free space on a
//! mounted filesystem.
//!
//! Shelling out to a system tool and parsing its JSON is the established
//! pattern here (`trib_audio::pulse` does the same with `pactl`): a pure
//! parser with fixture tests, `None` on garbage rather than a panic, and a
//! documented fallback when the tool is missing.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

/// Below this a volume is not a recording destination in any useful sense.
/// Eight tracks at 48 kHz/24-bit is roughly 69 MB per minute, so 1 GB is
/// about fifteen minutes of tape — and the 512 MB boot partition of a
/// flashed OS image, the case that prompted this, is seven.
const MIN_USABLE_BYTES: u64 = 1_000_000_000;

/// Mounts that are never recording destinations, whatever else is true —
/// system trees, snaps, AppImage mounts, container overlays. Only ever
/// applied to non-removable media: a removable drive is never silently
/// dropped, whatever it is mounted at.
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

/// Why a drive is or is not a usable destination.
///
/// Evaluated top-down, and the order is the point: these overlap (a
/// root-owned partition on the boot disk is both `System` and
/// `NotWritable`), so the winner is the reason the user can do least
/// about. "It is the boot disk" ends the conversation; "it is not
/// writable" invites a reformat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveState {
    /// Lives on the disk the running system booted from.
    System,
    /// The device or its mount is read-only.
    ReadOnly,
    /// Too small to record onto.
    TooSmall,
    /// No filesystem the kernel recognised — raw, LVM member, encrypted.
    UnknownFilesystem,
    /// Has a filesystem; nothing mounted it.
    NotMounted,
    /// Mounted read-write, but this uid cannot write to it.
    NotWritable,
    Ready,
}

impl DriveState {
    /// One short sentence for the console. The daemon owns the wording so
    /// the tile and the journal say the same thing.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            DriveState::System => Some("system disk — the appliance runs from it"),
            DriveState::ReadOnly => Some("mounted read-only"),
            DriveState::TooSmall => Some("too small to record onto"),
            DriveState::UnknownFilesystem => Some("no filesystem the appliance recognises"),
            DriveState::NotMounted => Some("connected, but nothing mounted it"),
            DriveState::NotWritable => Some("mounted, but the recorder cannot write to it"),
            DriveState::Ready => None,
        }
    }

    pub fn usable(self) -> bool {
        matches!(self, DriveState::Ready)
    }
}

/// How a drive is attached.
///
/// Description, never permission. This used to be policy: the automount
/// rule fired only for `ID_BUS=usb`, and the console refused to format
/// anything it had not called removable — which between them made an NVMe
/// SSD, the best medium this recorder can write to, unreachable. What
/// actually has to be refused is the disk the system booted from, and that
/// is `DriveState::System`, derived from the live mount table.
///
/// So this exists to pick an icon and print a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Usb,
    Nvme,
    /// An SD/MMC card, including the one the appliance boots from.
    Sd,
    Sata,
    /// Attached by something we have no better word for, or by nothing
    /// lsblk would name.
    Other,
}

impl Transport {
    /// lsblk's `TRAN`, which is a free-form string we deliberately narrow.
    fn from_tran(tran: Option<&str>) -> Self {
        match tran {
            Some("usb") => Self::Usb,
            Some("nvme") => Self::Nvme,
            Some("mmc") => Self::Sd,
            Some("sata" | "ata") => Self::Sata,
            _ => Self::Other,
        }
    }
}

/// One filesystem a recording could land on — mounted or not.
#[derive(Debug, Clone)]
pub struct Drive {
    pub device: Option<String>,
    /// `None` when the filesystem is present but nothing mounted it.
    pub mount_point: Option<String>,
    pub label: String,
    pub filesystem: Option<String>,
    pub total_bytes: u64,
    /// Only knowable while mounted.
    pub available_bytes: Option<u64>,
    pub removable: bool,
    /// How the drive is attached. Cosmetic: it names the icon and the word
    /// beside the size, and gates nothing.
    pub transport: Transport,
    pub state: DriveState,
    /// The whole disk this lives on, for grouping in the console.
    pub disk: Option<String>,
}

// ---------------------------------------------------------------- lsblk

/// One row of `lsblk --json`, as it comes off the wire.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Node {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, deserialize_with = "flexible_u64")]
    pub size: u64,
    #[serde(default)]
    pub fstype: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub mountpoints: Vec<Option<String>>,
    #[serde(default)]
    pub rm: bool,
    #[serde(default)]
    pub ro: bool,
    #[serde(default)]
    pub tran: Option<String>,
    /// The kernel's own "this can come and go" bit. Unlike `rm` (the SCSI
    /// removable bit, which plenty of sticks report as 0) it is set by the
    /// bus driver — but it is false for a USB stick on some kernels, so it
    /// is a third opinion rather than the answer.
    #[serde(default)]
    pub hotplug: bool,
    #[serde(default)]
    pub children: Vec<Node>,
}

impl Node {
    /// The real mount point, if any. lsblk reports `[SWAP]` in the same
    /// array as real paths, and null entries for some device types.
    pub fn mount(&self) -> Option<&str> {
        self.mountpoints
            .iter()
            .flatten()
            .map(String::as_str)
            .find(|m| m.starts_with('/'))
    }
}

/// `size` is a number under `-b` but a string on some builds. Accept both
/// rather than lose the whole enumeration to a formatting difference.
fn flexible_u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => Ok(n.as_u64().unwrap_or(0)),
        serde_json::Value::String(s) => Ok(s.trim().parse().unwrap_or(0)),
        _ => Ok(0),
    }
}

#[derive(serde::Deserialize)]
struct LsblkOutput {
    blockdevices: Vec<Node>,
}

/// Pure parse. `None` on anything unparseable — a broken `lsblk` must not
/// take the daemon down with it.
pub fn parse_lsblk(json: &str) -> Option<Vec<Node>> {
    serde_json::from_str::<LsblkOutput>(json)
        .ok()
        .map(|o| o.blockdevices)
}

/// Flatten to (node, parent-disk) pairs, dropping the pseudo-devices that
/// are never destinations.
fn walk(nodes: &[Node]) -> Vec<(&Node, Option<&Node>)> {
    let mut out = Vec::new();
    for disk in nodes {
        if disk.kind == "loop" || disk.name.starts_with("zram") {
            continue;
        }
        out.push((disk, None));
        for part in &disk.children {
            out.push((part, Some(disk)));
        }
    }
    out
}

/// Which disks the running system stands on. Derived from the live mount
/// table, never a hardcoded node name — the appliance is `mmcblk0`, a dev
/// box is `nvme0n1`, and a Pi booted from USB is `sda`.
pub fn system_disks(nodes: &[Node]) -> BTreeSet<String> {
    const SYSTEM_MOUNTS: [&str; 3] = ["/", "/boot", "/boot/firmware"];
    walk(nodes)
        .iter()
        .filter(|(node, _)| node.mount().is_some_and(|m| SYSTEM_MOUNTS.contains(&m)))
        .filter_map(|(node, disk)| {
            disk.map(|d| d.path.clone())
                .or_else(|| Some(node.path.clone()))
        })
        .collect()
}

/// Removability, resolved through the parent. `tran` is set on the parent
/// disk but null on the partitions of a USB stick — while an SD card and
/// an NVMe drive carry theirs on both — so a partition must ask its disk.
/// `rm` alone is unreliable: plenty of USB sticks report the SCSI
/// removable bit as 0, hence `hotplug` as a third opinion.
///
/// Nothing is refused on this any more. It picks the format dialog's
/// default filesystem and the tile's icon, and that is all: a fixed disk
/// is just as valid a place to record as a stick.
fn removable(node: &Node, disk: Option<&Node>) -> bool {
    let usb = |n: &Node| n.tran.as_deref() == Some("usb");
    usb(node)
        || disk.is_some_and(usb)
        || node.rm
        || disk.is_some_and(|d| d.rm)
        || node.hotplug
        || disk.is_some_and(|d| d.hotplug)
}

/// How this volume is attached, resolved through the parent for the same
/// reason removability is.
fn transport(node: &Node, disk: Option<&Node>) -> Transport {
    match Transport::from_tran(node.tran.as_deref()) {
        Transport::Other => Transport::from_tran(disk.and_then(|d| d.tran.as_deref())),
        known => known,
    }
}

/// The precedence table. `writable` is the probe's answer, `None` when the
/// volume is not mounted and the question was never asked.
pub fn classify(
    node: &Node,
    total_bytes: u64,
    on_system_disk: bool,
    writable: Option<bool>,
) -> DriveState {
    if on_system_disk {
        return DriveState::System;
    }
    if node.ro {
        return DriveState::ReadOnly;
    }
    if total_bytes < MIN_USABLE_BYTES {
        return DriveState::TooSmall;
    }
    if node.fstype.is_none() {
        return DriveState::UnknownFilesystem;
    }
    match writable {
        None => DriveState::NotMounted,
        Some(false) => DriveState::NotWritable,
        Some(true) => DriveState::Ready,
    }
}

// ------------------------------------------------------------ writability

/// Can this process actually write here?
///
/// The one writability truth in the codebase, and it is a real write
/// because that is the only answer that does not lie: a root-owned ext4
/// stick has `ro=false` on its mount and passes every flag-based check,
/// then fails at record time. Doing exactly what recording does — create,
/// write, remove — also catches a full disk and ACLs, which a permission
/// bit inspection would not.
///
/// Blocking. Costs one file create+unlink per mounted candidate.
pub fn writable(dir: &Path) -> bool {
    let probe = dir.join(".tribd-probe");
    std::fs::write(&probe, b"tributary")
        .and_then(|()| std::fs::remove_file(&probe))
        .is_ok()
}

/// What the tile prints. The mount basename is the friendly name when a
/// drive is mounted by label ("/media/FIELD_REC" → "FIELD_REC"); an
/// unmounted volume falls back to its filesystem label, then its node.
fn label_for(node: &Node) -> String {
    if let Some(mount) = node.mount()
        && let Some(name) = Path::new(mount).file_name()
    {
        return name.to_string_lossy().into_owned();
    }
    match node.label.as_deref() {
        Some(label) if !label.is_empty() => label.to_owned(),
        _ => node.path.clone(),
    }
}

fn excluded(mount: &str) -> bool {
    EXCLUDED_PREFIXES
        .iter()
        .any(|prefix| Path::new(mount).starts_with(prefix))
}

// -------------------------------------------------------------- enumerate

fn run_lsblk() -> Option<String> {
    let out = Command::new("lsblk")
        // NAME must stay in this list: lsblk only nests partitions under
        // `children` when it is selected, and emits a flat array without
        // it — which would turn every partition into a whole disk.
        .args([
            "--json",
            "-b",
            "-o",
            "NAME,PATH,TYPE,SIZE,FSTYPE,LABEL,MOUNTPOINTS,RM,RO,TRAN,HOTPLUG",
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Enumerate every candidate destination, usable first. Blocking (spawns
/// `lsblk`, runs statvfs, and write-probes each mounted candidate) — call
/// via `spawn_blocking`.
pub fn enumerate() -> Vec<Drive> {
    let Some(nodes) = run_lsblk().as_deref().and_then(parse_lsblk) else {
        tracing::warn!("lsblk unavailable — destination list will be empty");
        return Vec::new();
    };
    let free = free_space_by_mount();
    let system = system_disks(&nodes);

    let mut drives: Vec<Drive> = walk(&nodes)
        .into_iter()
        // A whole disk that has partitions is a container, not a volume.
        .filter(|(node, _)| node.children.is_empty())
        // Swap and other non-filesystem mounts are never destinations.
        .filter(|(node, _)| node.fstype.as_deref() != Some("swap"))
        .map(|(node, disk)| {
            let removable = removable(node, disk);
            let disk_path = disk.map(|d| d.path.clone());
            let on_system = disk_path
                .as_ref()
                .or(Some(&node.path))
                .is_some_and(|p| system.contains(p));
            let mount = node.mount().map(str::to_owned);
            let writable = mount.as_deref().map(|m| writable(Path::new(m)));
            let total = free
                .iter()
                .find(|(m, _, _)| Some(m.as_str()) == mount.as_deref())
                .map(|(_, total, _)| *total)
                .unwrap_or(node.size);
            Drive {
                state: classify(node, total.max(node.size), on_system, writable),
                available_bytes: free
                    .iter()
                    .find(|(m, _, _)| Some(m.as_str()) == mount.as_deref())
                    .map(|(_, _, avail)| *avail),
                total_bytes: total.max(node.size),
                device: Some(node.path.clone()),
                label: label_for(node),
                filesystem: node.fstype.clone(),
                mount_point: mount,
                removable,
                transport: transport(node, disk),
                disk: disk_path,
            }
        })
        // Non-removable system trees stay out; removable media never does.
        .filter(|d| d.removable || d.mount_point.as_deref().is_none_or(|m| !excluded(m)))
        .collect();

    drives.sort_by(|a, b| {
        b.state
            .usable()
            .cmp(&a.state.usable())
            .then_with(|| b.removable.cmp(&a.removable))
            .then_with(|| a.device.cmp(&b.device))
    });
    drives
}

/// (mount, total, available) for every mounted filesystem, via statvfs.
fn free_space_by_mount() -> Vec<(String, u64, u64)> {
    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| {
            (
                d.mount_point().display().to_string(),
                d.total_space(),
                d.available_space(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact stick that prompted this work, captured off the running
    /// appliance: a 57.8 GB drive holding a flashed OS image — 512 MB vfat
    /// `bootfs` and 3 GB ext4 `rootfs` — with ~54 GB unallocated.
    const APPLIANCE: &str = include_str!("../tests/fixtures/lsblk-appliance.json");

    /// A Pi 5 booting from its SD card with a 2 TB NVMe SSD on the M.2
    /// slot — the arrangement this feature exists for. Shaped from a real
    /// `lsblk` capture, including the detail that NVMe carries `tran` on
    /// the partitions as well as the disk, where USB carries it only on
    /// the disk.
    const NVME: &str = include_str!("../tests/fixtures/lsblk-nvme.json");

    fn nodes() -> Vec<Node> {
        parse_lsblk(APPLIANCE).expect("fixture parses")
    }

    fn nvme_nodes() -> Vec<Node> {
        parse_lsblk(NVME).expect("fixture parses")
    }

    #[test]
    fn garbage_is_none_not_a_panic() {
        assert!(parse_lsblk("").is_none());
        assert!(parse_lsblk("{}").is_none());
        assert!(parse_lsblk(r#"{"blockdevices":"nope"}"#).is_none());
    }

    #[test]
    fn the_flashed_image_stick_parses() {
        let nodes = nodes();
        let sda = nodes.iter().find(|n| n.name == "sda").unwrap();
        // Nesting is what tells a partition from a disk, and it only
        // happens because NAME is in the -o list.
        assert!(!sda.children.is_empty(), "lsblk must nest partitions");
        assert_eq!(sda.size, 62_026_416_128);
        assert_eq!(sda.children.len(), 2);
        assert_eq!(sda.children[0].label.as_deref(), Some("bootfs"));
        assert_eq!(sda.children[0].mount(), Some("/media/bootfs"));
    }

    /// Both partitions of the stick are labelled identically to the SD
    /// card's. Identity has to come from the device node, never the label.
    #[test]
    fn duplicate_labels_across_disks_stay_distinct() {
        let nodes = nodes();
        let bootfs: Vec<_> = walk(&nodes)
            .into_iter()
            .filter(|(n, _)| n.label.as_deref() == Some("bootfs"))
            .map(|(n, _)| n.path.as_str())
            .collect();
        assert_eq!(bootfs, ["/dev/sda1", "/dev/mmcblk0p1"]);
    }

    #[test]
    fn the_boot_disk_is_found_from_the_mount_table() {
        assert_eq!(
            system_disks(&nodes()),
            BTreeSet::from(["/dev/mmcblk0".to_owned()])
        );
    }

    /// `tran` is "usb" on the parent and null on the partitions, so a
    /// partition that asked only about itself would look internal.
    #[test]
    fn removability_resolves_through_the_parent() {
        let nodes = nodes();
        for (node, disk) in walk(&nodes) {
            let want = node.path.starts_with("/dev/sda");
            if node.path.starts_with("/dev/sda") || node.path.starts_with("/dev/mmcblk") {
                assert_eq!(removable(node, disk), want, "{}", node.path);
            }
        }
    }

    #[test]
    fn swap_and_pseudo_devices_are_not_candidates() {
        let names: Vec<_> = walk(&nodes()).iter().map(|(n, _)| n.name.clone()).collect();
        assert!(!names.contains(&"loop0".to_owned()));
        assert!(!names.contains(&"zram0".to_owned()));
    }

    fn node_with(fstype: Option<&str>, ro: bool) -> Node {
        Node {
            name: "sdb1".into(),
            path: "/dev/sdb1".into(),
            kind: "part".into(),
            size: 0,
            fstype: fstype.map(str::to_owned),
            label: None,
            mountpoints: vec![],
            rm: true,
            ro,
            tran: None,
            hotplug: false,
            children: vec![],
        }
    }

    /// The overlaps are the whole reason the order is written down.
    #[test]
    fn classification_precedence() {
        let big = 64_000_000_000;
        let ext4 = node_with(Some("ext4"), false);
        let ro = node_with(Some("ext4"), true);

        // System beats everything, including unwritable and read-only.
        assert_eq!(classify(&ro, 0, true, Some(false)), DriveState::System);
        // Read-only beats too-small.
        assert_eq!(classify(&ro, 1, false, None), DriveState::ReadOnly);
        // Too-small beats an unknown filesystem: a 512 MB partition is not
        // worth explaining a filesystem for.
        assert_eq!(
            classify(&node_with(None, false), 1, false, None),
            DriveState::TooSmall
        );
        assert_eq!(
            classify(&node_with(None, false), big, false, None),
            DriveState::UnknownFilesystem
        );
        assert_eq!(classify(&ext4, big, false, None), DriveState::NotMounted);
        assert_eq!(
            classify(&ext4, big, false, Some(false)),
            DriveState::NotWritable
        );
        assert_eq!(classify(&ext4, big, false, Some(true)), DriveState::Ready);
    }

    #[test]
    fn every_unusable_state_explains_itself() {
        for state in [
            DriveState::System,
            DriveState::ReadOnly,
            DriveState::TooSmall,
            DriveState::UnknownFilesystem,
            DriveState::NotMounted,
            DriveState::NotWritable,
        ] {
            assert!(!state.usable());
            assert!(state.reason().is_some_and(|r| !r.is_empty()), "{state:?}");
        }
        assert!(DriveState::Ready.usable());
        assert!(DriveState::Ready.reason().is_none());
    }

    #[test]
    fn labels_prefer_the_mount_basename_then_the_volume_label() {
        let mut n = node_with(Some("vfat"), false);
        n.label = Some("FIELD".into());
        assert_eq!(label_for(&n), "FIELD");
        n.mountpoints = vec![Some("/media/FIELD_REC".into())];
        assert_eq!(label_for(&n), "FIELD_REC");
        let bare = node_with(Some("vfat"), false);
        assert_eq!(label_for(&bare), "/dev/sdb1");
    }

    /// A path that cannot be created under any uid — deterministic even
    /// when the suite runs as root, which a 0o555 tempdir would not be.
    #[test]
    fn writable_says_no_for_an_impossible_path() {
        assert!(!writable(Path::new("/proc/tributary-no-such-place")));
    }

    #[test]
    fn writable_says_yes_for_a_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(writable(dir.path()));
        // The probe leaves nothing behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Real-system smoke test: whatever is mounted, the root filesystem is
    /// present and it is reported as the system disk, never as a tile the
    /// user could pick by accident.
    #[test]
    fn enumerate_marks_the_root_filesystem_as_system() {
        let drives = enumerate();
        if let Some(root) = drives
            .iter()
            .find(|d| d.mount_point.as_deref() == Some("/"))
        {
            assert_eq!(root.state, DriveState::System);
        }
        let usable = drives.iter().position(|d| !d.state.usable());
        if let Some(first_unusable) = usable {
            assert!(
                drives[..first_unusable].iter().all(|d| d.state.usable()),
                "usable drives sort first: {drives:?}"
            );
        }
    }

    // ------------------------------------------------------------- NVMe

    #[test]
    fn the_nvme_fixture_nests_like_any_other_disk() {
        let nodes = nvme_nodes();
        let nvme = nodes.iter().find(|n| n.name == "nvme0n1").unwrap();
        assert_eq!(nvme.children.len(), 1);
        // p1, not 1 — the naming that made `${DEV}1` wrong in the helper.
        assert_eq!(nvme.children[0].path, "/dev/nvme0n1p1");
        assert_eq!(nvme.children[0].mount(), Some("/media/TRIBUTARY"));
    }

    /// The whole point. An attached NVMe is an ordinary candidate, and the
    /// SD card it booted from is not — decided by the mount table, not by
    /// how either one is plugged in.
    #[test]
    fn an_attached_nvme_is_a_candidate_and_the_boot_card_is_not() {
        let nodes = nvme_nodes();
        let system = system_disks(&nodes);
        assert!(
            system.contains("/dev/mmcblk0"),
            "boot card is the system disk"
        );
        assert!(!system.contains("/dev/nvme0n1"), "attached NVMe is not");

        let pairs = walk(&nodes);
        let (part, disk) = pairs
            .iter()
            .find(|(n, _)| n.path == "/dev/nvme0n1p1")
            .copied()
            .unwrap();
        assert_eq!(
            classify(part, part.size, false, Some(true)),
            DriveState::Ready,
            "a mounted, writable NVMe volume is ready to record to"
        );
        assert_eq!(transport(part, disk), Transport::Nvme);
    }

    /// Boot from the NVMe instead and the answer flips, with no change to
    /// anything about the drive itself. A transport test could never do
    /// this: it would have called the same disk formattable either way.
    ///
    /// The mount is moved on the parsed nodes rather than in the fixture
    /// text — the assertion is about where `/` lives, and editing JSON by
    /// substring would make it about whitespace.
    #[test]
    fn booting_from_the_nvme_makes_it_the_system_disk() {
        let mut nodes = nvme_nodes();
        for disk in &mut nodes {
            for part in &mut disk.children {
                part.mountpoints = match part.path.as_str() {
                    "/dev/nvme0n1p1" => vec![Some("/".into())],
                    "/dev/mmcblk0p2" => vec![None],
                    _ => std::mem::take(&mut part.mountpoints),
                };
            }
        }
        let system = system_disks(&nodes);
        assert!(system.contains("/dev/nvme0n1"), "now the system disk");
        // And the card stays one too, because it still holds
        // /boot/firmware — which is exactly the Pi 5 arrangement where the
        // firmware lives on the SD and the root filesystem on the SSD.
        // Both disks are off limits, and for the same stated reason.
        assert!(
            system.contains("/dev/mmcblk0"),
            "a disk carrying /boot/firmware is a system disk even when / is elsewhere"
        );

        let pairs = walk(&nodes);
        let (part, _) = pairs
            .iter()
            .find(|(n, _)| n.path == "/dev/nvme0n1p1")
            .copied()
            .unwrap();
        assert_eq!(
            classify(part, part.size, true, Some(true)),
            DriveState::System,
            "System wins the precedence order over every other state"
        );
    }

    /// Transport resolves through the parent, because USB reports it only
    /// on the disk. It names an icon and nothing else.
    #[test]
    fn transport_resolves_through_the_parent() {
        let nodes = nodes();
        let pairs = walk(&nodes);
        let usb_part = pairs.iter().find(|(n, _)| n.path == "/dev/sda1").unwrap();
        assert_eq!(usb_part.0.tran, None, "USB leaves it null on partitions");
        assert_eq!(transport(usb_part.0, usb_part.1), Transport::Usb);

        let sd_part = pairs
            .iter()
            .find(|(n, _)| n.path == "/dev/mmcblk0p2")
            .unwrap();
        assert_eq!(transport(sd_part.0, sd_part.1), Transport::Sd);
    }

    /// A fixed disk is still removable-for-display purposes only. The bit
    /// must not creep back into a decision: it picks an icon and the
    /// format dialog's default filesystem.
    #[test]
    fn a_fixed_nvme_is_not_removable_but_is_still_a_candidate() {
        let nodes = nvme_nodes();
        let pairs = walk(&nodes);
        let (part, disk) = pairs
            .iter()
            .find(|(n, _)| n.path == "/dev/nvme0n1p1")
            .copied()
            .unwrap();
        assert!(!removable(part, disk));
        assert_eq!(
            classify(part, part.size, false, Some(true)),
            DriveState::Ready
        );
    }

    /// `hotplug` is the third opinion, for the sticks that report the SCSI
    /// removable bit as 0 and carry no `usb` transport of their own.
    #[test]
    fn hotplug_alone_is_enough_to_call_a_drive_removable() {
        let mut node = node_with(Some("exfat"), false);
        node.rm = false;
        node.tran = None;
        assert!(!removable(&node, None));
        node.hotplug = true;
        assert!(removable(&node, None));
    }
}
