#!/usr/bin/env bash
# Wipe an attached drive and lay down one volume spanning all of it.
#
# This is the appliance's one privileged escape hatch. tribd runs as a
# systemd USER unit under a nologin service account with no capabilities,
# and partitioning needs CAP_SYS_ADMIN — so a sudoers drop-in grants
# exactly this script, and everything below the argument parse is the
# refusal list that makes that grant safe.
#
# Never trust the caller. The caller is a daemon reachable by anyone on the
# appliance's open Wi-Fi AP, so every fact is re-derived from the kernel
# here rather than believed. The daemon validates too; these are the
# checks that hold even if it does not.
#
# Any drive, not just USB. The bus a drive hangs off is a description, not
# a permission: an NVMe SSD on a Pi 5's M.2 slot is the best medium this
# recorder can write to. What actually has to be refused is the disk the
# appliance runs from, and that is asked directly (blockdev-lib.sh) rather
# than approximated by `ID_BUS=="usb"`.
#
# Two filesystems, because the right answer depends on where the drive
# lives. exFAT for something you unplug and open on a laptop; ext4 for a
# drive that stays in the recorder, where journalling and real ownership
# are worth more than being readable on macOS.
#
# Exit codes are the daemon's HTTP mapping:
#   0 ok · 2 refused/invalid (422) · 3 busy (409) · 1 internal (500)
set -euo pipefail

readonly MEDIA_ROOT="${MEDIA_ROOT:-/media}"
readonly TRIB_USER="${TRIB_USER:-tributary}"

# shellcheck source=scripts/pi-image/blockdev-lib.sh
. "$(dirname "$(readlink -f "$0")")/blockdev-lib.sh"

readonly E_REFUSED=2
readonly E_BUSY=3

# An MBR partition table addresses 2^32 512-byte sectors and no more, so a
# drive past this gets GPT whatever filesystem it is carrying. Silently
# handing back 2 TiB of a 4 TB drive is not a compatibility trade, it is
# data loss with a friendly face.
readonly MBR_MAX_SECTORS=4294967295

# Partition type GUIDs, spelled out rather than using sfdisk's letter
# shortcuts: the shortcut table has changed across util-linux versions and
# this script runs on whatever the base image ships.
readonly GPT_LINUX=0FC63DAF-8483-4772-8E79-3D69D8477DE4
readonly GPT_MSDATA=EBD0A0A2-B9E5-4433-87C0-68B6B72699C7

die() { echo "$2" >&2; exit "$1"; }

# 11 characters is exFAT's volume label limit — a filesystem fact, not a
# style choice. ext4 would allow 16, and it still gets 11: one rule that
# the helper, the daemon and the console can all state identically is worth
# more than five extra characters on one of the two filesystems. ASCII
# only, because this string becomes the volume label, then the mount
# basename, then the tile: validating it here means the automounter's
# sanitiser is a no-op on drives we formatted ourselves.
validate_label() {
    case "$1" in
    '') return 1 ;;
    *[!A-Za-z0-9_.-]*) return 1 ;;
    esac
    [ "${#1}" -le 11 ]
}

# The filesystems this helper knows how to lay down. Adding one means
# adding a row here and an arm in the format block below — and nothing else.
validate_fstype() {
    case "$1" in
    exfat | ext4) return 0 ;;
    *) return 1 ;;
    esac
}

case "${1:-}" in
--print-label-check)
    validate_label "${2:-}" && echo ok || echo refused
    exit 0
    ;;
--print-fs-check)
    validate_fstype "${2:-}" && echo ok || echo refused
    exit 0
    ;;
# Takes a major:minor rather than a device node so it can be exercised
# against a fixture $SYS_BLOCK tree and $MOUNTINFO file, with no root and
# no real disk.
--print-system-check)
    is_system_devno "${2:-}" && echo system || echo other
    exit 0
    ;;
--print-part-node)
    part_node "${2:-}"
    exit 0
    ;;
esac

if [ "${1:-}" = self-check ]; then
    # Proves the whole chain in one call: this script is reachable through
    # sudo AND every tool it needs exists. A check that only looked at the
    # file would lie about the sudoers rule.
    for t in /sbin/wipefs /sbin/sfdisk /sbin/mkfs.exfat /sbin/mkfs.ext4 \
        /usr/bin/lsblk /usr/bin/udevadm /usr/bin/systemd-umount; do
        [ -x "$t" ] || die 1 "format-drive: missing $t"
    done
    [ "$(id -u)" = 0 ] || die 1 "format-drive: not running as root"
    echo ok
    exit 0
fi

[ "${1:-}" = format ] \
    || die "$E_REFUSED" "usage: format-drive.sh {self-check|format <devnode> <label> [fstype]}"
DEV="${2:?device required}"
LABEL="${3:?label required}"
# Defaulted, not required: a daemon predating the filesystem picker sends
# two arguments and means exactly what this script used to do.
FSTYPE="${4:-exfat}"

validate_label "$LABEL" \
    || die "$E_REFUSED" "label must be 1-11 characters of A-Z a-z 0-9 . _ -"
validate_fstype "$FSTYPE" \
    || die "$E_REFUSED" "unknown filesystem: $FSTYPE"

# ---- refusals, in the order that matters -------------------------------

case "$DEV" in
/dev/*) ;;
*) die "$E_REFUSED" "not a device node: $DEV" ;;
esac
[ -b "$DEV" ] || die "$E_REFUSED" "not a block device: $DEV"

devno="$(devno_of "$DEV")"
# Whole disks only. Formatting a partition would leave the rest of the
# drive stranded, which is the exact problem this feature exists to fix.
[ -e "$SYS_BLOCK/$devno/partition" ] \
    && die "$E_REFUSED" "$DEV is a partition — format the whole drive"

# The guard that replaced the USB test, and the reason dropping it is safe.
is_system_devno "$devno" \
    && die "$E_REFUSED" "$DEV is the disk the appliance runs from"

# Anything of this drive mounted outside /media was mounted by someone
# else for a reason we do not know. Our own /media mounts we stop
# properly, so the transient units go away instead of being orphaned.
while read -r mnt_devno mnt_point; do
    [ "$(parent_devno "$mnt_devno")" = "$devno" ] || continue
    case "$mnt_point" in
    "$MEDIA_ROOT"/*)
        systemd-umount "$mnt_point" \
            || die "$E_BUSY" "$mnt_point is in use — stop recording and try again"
        ;;
    *) die "$E_REFUSED" "$DEV is mounted at $mnt_point — not ours to erase" ;;
    esac
done < <(awk '{ print $3, $5 }' "$MOUNTINFO")

uid="$(id -u "$TRIB_USER")" || die 1 "no such user: $TRIB_USER"
gid="$(id -g "$TRIB_USER")" || die 1 "no such user: $TRIB_USER"

# ---- the destructive part ----------------------------------------------

echo "stage: partitioning" >&2

# One partition spanning the drive. The table type follows the size first
# and the filesystem second: ext4 has no reason to want MBR, and exFAT only
# does for compatibility with things that cannot read GPT anyway.
sectors="$(cat "$SYS_BLOCK/$devno/size")"
if [ "$FSTYPE" = ext4 ] || [ "$sectors" -gt "$MBR_MAX_SECTORS" ]; then
    table=gpt
    case "$FSTYPE" in
    ext4) parttype="$GPT_LINUX" ;;
    *) parttype="$GPT_MSDATA" ;;
    esac
else
    table=dos
    parttype=7
fi

wipefs -a "$DEV" >/dev/null
echo ",,$parttype" | sfdisk --quiet --label "$table" "$DEV"
udevadm settle

# Asked of the kernel, never spelled: sda1 but nvme0n1p1.
part="$(part_node "$DEV")"
[ -n "$part" ] && [ -b "$part" ] \
    || die 1 "no partition appeared on $DEV after partitioning"

echo "stage: formatting" >&2
case "$FSTYPE" in
exfat)
    # No POSIX ownership on disk; the automounter's uid=/gid= options are
    # what make the result writable.
    mkfs.exfat -L "$LABEL" "$part" >/dev/null
    ;;
ext4)
    # root_owner is the whole reason ext4 is offered at all. ext4 *does*
    # carry ownership on disk, so without this the volume comes back owned
    # by root, the daemon's write probe honestly reports "not writable",
    # and the console shows a drive the user just formatted as unusable.
    # Setting it at creation leaves no window where that is briefly true.
    #
    # -m 0 because the 5% reserved-blocks default is for a root filesystem
    # that must not wedge; on a 2 TB recording drive it is 100 GB of takes.
    mkfs.ext4 -q -L "$LABEL" -m 0 -E "root_owner=$uid:$gid" "$part" >/dev/null
    ;;
esac

# The kernel usually emits `change` when mkfs closes the device, which the
# automount rule matches — but ask explicitly so the drive comes back
# mounted whether or not it did.
echo "stage: mounting" >&2
udevadm trigger --action=change --name-match="$part" || true
udevadm settle

echo "stage: done" >&2
echo "$part"
