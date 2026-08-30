#!/usr/bin/env bash
# Mount an attached filesystem so the console can offer it as a recording
# destination. Raspberry Pi OS Lite ships no automounter and no desktop, so
# without this a plugged-in drive reaches the kernel and stops there: tribd
# enumerates the mount table, and an unmounted device is invisible to it by
# construction.
#
# Driven by 99-tributary-storage.rules, which fires for any block device
# carrying a filesystem — USB stick, NVMe SSD, SD card in a reader. udev
# kills long-running RUN processes, so everything here must return
# immediately — hence `systemd-mount --no-block`, which hands the work to
# PID 1 and exits.
#
# The rule used to filter on ID_BUS=usb and no longer does, so the refusal
# that keeps the boot media out lives here now (see the system-disk guard
# below). That is the trade: the rule got simpler and this script took the
# responsibility, in the one place that can actually ask the question.
#
# udev already probed the device, so the rule passes ID_FS_TYPE/ID_FS_LABEL
# in and this script never re-runs blkid.
set -euo pipefail

readonly MOUNT_ROOT="${MOUNT_ROOT:-/media}"
readonly TRIB_USER="${TRIB_USER:-tributary}"

# shellcheck source=scripts/pi-image/blockdev-lib.sh
. "$(dirname "$(readlink -f "$0")")/blockdev-lib.sh"

# Absolute paths throughout: udev and systemd hand us a minimal PATH that
# does not include /sbin, and a bare `findmnt` there fails silently-ish.
readonly FINDMNT="${FINDMNT:-/usr/bin/findmnt}"
readonly MOUNTPOINT_BIN="${MOUNTPOINT_BIN:-/usr/bin/mountpoint}"
readonly SYSTEMD_MOUNT="${SYSTEMD_MOUNT:-/usr/bin/systemd-mount}"

# udev throws away a RUN program's stdout and stderr unless udev itself is
# in debug mode, which is exactly how "nothing mounted it and nothing said
# why" happens — the failure shape this project has been bitten by before.
# Everything worth reading goes to the journal under a greppable tag:
#   journalctl -t tributary-storage
log() {
    logger -t tributary-storage -p daemon.info -- "$*" 2>/dev/null || true
}

# blkid reports plain "ntfs", and Debian routes `mount -t ntfs` to
# /sbin/mount.ntfs — ntfs-3g over FUSE, whose ownership and allow_other
# semantics are a separate problem. The in-kernel ntfs3 driver takes the
# same uid=/gid=/mask options as vfat and exfat, so one code path covers
# all three.
mount_type() {
    case "$1" in
    ntfs) printf 'ntfs3' ;;
    *) printf '%s' "$1" ;;
    esac
}

# How the daemon gets write access. vfat/exfat/ntfs carry no POSIX
# ownership, so ownership is a *mount option* — without it every file lands
# as root and tributary (uid 999) cannot write, which the console would then
# report as a read-only drive. fmask/dmask rather than umask: files 0664,
# directories 0775.
#
# Never `sync`: it collapses sustained write throughput, which is fatal for
# multitrack audio. Never chown a foreign filesystem to make ext4 writable —
# a root-owned stick stays unwritable on purpose, and the console says so.
mount_options() {
    local fstype="$1" uid="$2" gid="$3"
    local common="noatime,nosuid,nodev,noexec"
    case "$fstype" in
    # utf8=1 is load-bearing, not decoration: the kernel's default vfat
    # iocharset is ascii here (verified on the appliance), and tribd writes
    # project and take directories from user-typed names.
    vfat)
        printf '%s,uid=%s,gid=%s,fmask=0113,dmask=0002,utf8=1' "$common" "$uid" "$gid"
        ;;
    exfat | ntfs | ntfs3)
        printf '%s,uid=%s,gid=%s,fmask=0113,dmask=0002' "$common" "$uid" "$gid"
        ;;
    # Passing uid= to ext4 does not merely get ignored — the mount fails.
    *)
        printf '%s' "$common"
        ;;
    esac
}

# The mount basename becomes the tile's printed name in the console
# (tribd's label_for() reads it), so keep it recognisable rather than
# aggressively encoded. Anything that could escape /media or confuse a path
# is folded to underscore; an unlabelled volume falls back to its kernel
# name (sda1).
sanitize_label() {
    local label="$1" fallback="$2" clean
    clean="$(printf '%s' "$label" | tr -c 'A-Za-z0-9._-' '_' \
        | sed 's/_\{2,\}/_/g; s/^[._-]*//; s/[._-]*$//')"
    clean="${clean:0:32}"
    [ -n "$clean" ] || clean="$fallback"
    printf '%s' "$clean"
}

# A directory holding anything is either a live mount or somebody's data —
# mounting over it would shadow it. An empty leftover from a previous
# insertion is ours to reuse.
taken() {
    [ -n "$(ls -A "$1" 2>/dev/null || true)" ] && return 0
    "$MOUNTPOINT_BIN" -q "$1" 2>/dev/null
}

reserve_mountpoint() {
    local name="$1" candidate n=2
    mkdir -p "$MOUNT_ROOT"
    candidate="$MOUNT_ROOT/$name"
    while true; do
        # mkdir without -p IS the lock: it fails when the name is already
        # taken, so two udev workers racing two identically-labelled sticks
        # resolve deterministically instead of both claiming /media/DATA.
        if mkdir "$candidate" 2>/dev/null; then
            printf '%s' "$candidate"
            return
        fi
        # Our own empty leftover from a previous insertion is ours to reuse.
        if [ -d "$candidate" ] && ! taken "$candidate"; then
            printf '%s' "$candidate"
            return
        fi
        candidate="$MOUNT_ROOT/$name-$n"
        n=$((n + 1))
    done
}

# Fixture modes, so both derivations can be exercised on a dev box with no
# Pi, no root and no image — the same trick ap-prepare.sh uses for its SSID.
case "${1:-}" in
--print-options)
    mount_options "${2:-}" "${3:-}" "${4:-}"
    exit
    ;;
--print-mountpoint)
    reserve_mountpoint "$(sanitize_label "${2:-}" "${3:-}")"
    exit
    ;;
--print-type)
    mount_type "${2:-}"
    exit
    ;;
# Takes a major:minor rather than a device node so the guard can be
# exercised against a fixture $SYS_BLOCK tree and $MOUNTINFO file, with no
# root and no real disk.
--print-system-check)
    is_system_devno "${2:-}" && echo system || echo other
    exit
    ;;
esac

readonly DEV="${1:?usage: drive-mount.sh <devnode> <fstype> [label]}"
readonly FSTYPE="${2:?usage: drive-mount.sh <devnode> <fstype> [label]}"
readonly LABEL="${3:-}"

# Never touch the disk the appliance runs from. Since the udev rule stopped
# filtering on ID_BUS, this is what keeps the boot media off the console's
# destination list — and unlike the bus test it holds however the Pi booted,
# from SD, from USB or from NVMe.
#
# Belt to the braces below: an unmounted spare partition on the boot disk
# would sail straight past the already-mounted guard.
if is_system_device "$DEV"; then
    log "$DEV is on the disk the appliance runs from — leaving it alone"
    exit 0
fi

# The second guard, and it covers more than it looks like. The rule matches
# `change` as well as `add`, so it fires repeatedly for the same device —
# including for a booted-from drive's own live partitions. Re-mounting a
# live filesystem elsewhere would be at best confusing and at worst
# destructive.
if "$FINDMNT" -n -S "$DEV" >/dev/null 2>&1; then
    log "$DEV is already mounted — leaving it alone"
    exit 0
fi

uid="$(id -u "$TRIB_USER")"
gid="$(id -g "$TRIB_USER")"

fstype="$(mount_type "$FSTYPE")"
target="$(reserve_mountpoint "$(sanitize_label "$LABEL" "$(basename "$DEV")")")"
options="$(mount_options "$fstype" "$uid" "$gid")"

log "mounting $DEV ($fstype, label '${LABEL:-none}') at $target"

# --no-block because udev reaps us if we linger; --collect so a failed
# attempt does not leave a `failed` unit behind to block the next insertion
# at the same path until someone runs `systemctl reset-failed`.
#
# Teardown on unplug is free and was verified on hardware: the transient
# unit comes out carrying BindsTo=dev-sdXN.device and StopPropagatedFrom on
# the same, so no remove rule is needed. Pass the real devnode, never a
# /dev/disk/by-id symlink — sysinfo decides `removable` by comparing
# /proc/mounts' source against the canonicalised by-id targets, and a
# symlink there silently turns the console's transport label into a guess.
exec "$SYSTEMD_MOUNT" --no-block --collect \
    --type="$fstype" --options="$options" \
    -- "$DEV" "$target"
