#!/usr/bin/env bash
# Wipe a USB drive and lay down one exFAT volume spanning all of it.
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
# exFAT because the point of a stick on a recorder is to unplug it and
# open the files on a laptop — ext4 needs third-party software on macOS
# and Windows. It also has no POSIX ownership on disk, so the automounter's
# uid=/gid= options are what make the result writable.
#
# Exit codes are the daemon's HTTP mapping:
#   0 ok · 2 refused/invalid (422) · 3 busy (409) · 1 internal (500)
set -euo pipefail

readonly MEDIA_ROOT="${MEDIA_ROOT:-/media}"
readonly MOUNTINFO="${MOUNTINFO:-/proc/self/mountinfo}"
readonly SYS_BLOCK="${SYS_BLOCK:-/sys/dev/block}"

readonly E_REFUSED=2
readonly E_BUSY=3

die() { echo "$2" >&2; exit "$1"; }

# exFAT's volume label limit is 11 characters — a filesystem fact, not a
# style choice. ASCII only, because this string becomes the volume label,
# then the mount basename, then the tile: validating it here means the
# automounter's sanitiser is a no-op on drives we formatted ourselves.
validate_label() {
    case "$1" in
    '') return 1 ;;
    *[!A-Za-z0-9_.-]*) return 1 ;;
    esac
    [ "${#1}" -le 11 ]
}

# Decimal major:minor, the form mountinfo field 3 uses. (%t:%T is hex.)
devno_of() { stat -Lc '%Hr:%Lr' -- "$1"; }

# 8:1 -> 8:0. /sys/dev/block/<devno>/partition exists iff it is a
# partition, and its parent directory is the whole disk.
parent_devno() {
    if [ -e "$SYS_BLOCK/$1/partition" ]; then
        cat "$(readlink -f "$SYS_BLOCK/$1/..")/dev"
    else
        printf '%s' "$1"
    fi
}

# The disks the running system stands on, as major:minor of the whole
# disk. Derived live: the appliance is mmcblk0, a dev box is nvme0n1, and
# a Pi booted from USB is sda.
system_disks() {
    awk '$5 == "/" || $5 == "/boot" || $5 == "/boot/firmware" { print $3 }' "$MOUNTINFO" \
        | while read -r devno; do parent_devno "$devno"; done | sort -u
}

if [ "${1:-}" = --print-label-check ]; then
    validate_label "${2:-}" && echo ok || echo refused
    exit 0
fi

if [ "${1:-}" = self-check ]; then
    # Proves the whole chain in one call: this script is reachable through
    # sudo AND every tool it needs exists. A check that only looked at the
    # file would lie about the sudoers rule.
    for t in /sbin/wipefs /sbin/sfdisk /sbin/mkfs.exfat /usr/bin/udevadm /usr/bin/systemd-umount; do
        [ -x "$t" ] || die 1 "format-drive: missing $t"
    done
    [ "$(id -u)" = 0 ] || die 1 "format-drive: not running as root"
    echo ok
    exit 0
fi

[ "${1:-}" = format ] || die "$E_REFUSED" "usage: format-drive.sh {self-check|format <devnode> <label>}"
DEV="${2:?device required}"
LABEL="${3:?label required}"

validate_label "$LABEL" \
    || die "$E_REFUSED" "label must be 1-11 characters of A-Z a-z 0-9 . _ -"

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

# Re-read the bus NOW rather than trusting anything the caller enumerated
# earlier: /dev/sdX is racy across a replug.
bus="$(udevadm info --query=property --name="$DEV" 2>/dev/null | sed -n 's/^ID_BUS=//p')"
[ "$bus" = usb ] || die "$E_REFUSED" "$DEV is not a USB drive"

for sys in $(system_disks); do
    [ "$sys" = "$devno" ] && die "$E_REFUSED" "$DEV is the disk the appliance runs from"
done

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

# ---- the destructive part ----------------------------------------------

echo "stage: partitioning" >&2
wipefs -a "$DEV" >/dev/null
# One primary partition, type 07 (exFAT/NTFS), spanning the drive. MBR
# rather than GPT purely for removable-media compatibility breadth.
echo ',,7' | sfdisk --quiet --label dos "$DEV"
udevadm settle

part="${DEV}1"
[ -b "$part" ] || die 1 "expected $part after partitioning"

echo "stage: formatting" >&2
mkfs.exfat -L "$LABEL" "$part" >/dev/null

# The kernel usually emits `change` when mkfs closes the device, which the
# automount rule matches — but ask explicitly so the drive comes back
# mounted whether or not it did.
echo "stage: mounting" >&2
udevadm trigger --action=change --name-match="$part" || true
udevadm settle

echo "stage: done" >&2
echo "$part"
