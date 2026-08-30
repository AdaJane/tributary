#!/usr/bin/env bash
# Block-device facts shared by the Tributary storage helpers.
#
# Sourced, never executed: drive-mount.sh and format-drive.sh both need to
# answer "is this the disk the appliance runs from?", and that question must
# have exactly one implementation. It is the only thing standing between a
# transport-blind udev rule and someone reformatting the boot media.
#
# Everything here is derived from the kernel at the moment it is asked. No
# fact about a device is ever carried in from the caller: /dev/sdX is racy
# across a replug, and this library runs under a sudoers grant.
#
# The three env seams exist so every derivation below can be exercised
# against fixtures on a dev box with no root, no Pi and no real disks --
# the same trick ap-prepare.sh uses for its SSID.

: "${MOUNTINFO:=/proc/self/mountinfo}"
: "${SYS_BLOCK:=/sys/dev/block}"
: "${LSBLK:=/usr/bin/lsblk}"

# Decimal major:minor, the form mountinfo field 3 uses. (%t:%T is hex.)
devno_of() { stat -Lc '%Hr:%Lr' -- "$1"; }

# 8:1 -> 8:0. /sys/dev/block/<devno>/partition exists iff it is a
# partition, and its parent directory is the whole disk. A devno that is
# already a whole disk answers itself, so callers never branch.
#
# Both branches emit exactly one newline-terminated line, which is not
# cosmetic: system_disks() runs this in a loop, and the original wrote the
# whole-disk answer with a bare `printf %s` while the partition answer came
# newline-terminated out of sysfs via cat. Two system mounts resolving down
# different branches concatenated into one meaningless devno, and a
# system_disks() that names nothing real fails *open* — the boot disk stops
# looking like the boot disk. Normalising through a variable also makes the
# derivation indifferent to whether sysfs terminated the value at all.
parent_devno() {
    local devno="$1"
    if [ -e "$SYS_BLOCK/$devno/partition" ]; then
        devno="$(cat "$(readlink -f "$SYS_BLOCK/$devno/..")/dev")"
    fi
    printf '%s\n' "$devno"
}

# The disks the running system stands on, as major:minor of the whole
# disk. Derived live: the appliance is mmcblk0, a dev box is nvme0n1, and
# a Pi booted from USB is sda.
system_disks() {
    awk '$5 == "/" || $5 == "/boot" || $5 == "/boot/firmware" { print $3 }' "$MOUNTINFO" \
        | while read -r devno; do parent_devno "$devno"; done | sort -u
}

# Does this major:minor belong to the system disk -- either as the disk
# itself or as one of its partitions?
#
# This replaced `ID_BUS=="usb"` as the thing that keeps the boot media off
# the destination list. The bus test was a proxy that happened to exclude
# the SD card; this asks the actual question, which is why it also holds
# for a Pi booted from USB or from NVMe.
is_system_devno() {
    local parent sys
    parent="$(parent_devno "$1")"
    while read -r sys; do
        [ "$sys" = "$parent" ] && return 0
    done < <(system_disks)
    return 1
}

# The same question about a device node.
is_system_device() {
    local devno
    devno="$(devno_of "$1")" || return 1
    is_system_devno "$devno"
}

# The first partition of a whole disk, asked of the kernel rather than
# spelled.
#
# Name arithmetic is what made this USB-only: `${DEV}1` is sda1 but
# nvme0n11, and mmcblk01. Reading it back removes the class of bug instead
# of adding a second case for every future naming scheme.
part_node() {
    "$LSBLK" --noheadings --raw --output PATH,TYPE -- "$1" 2>/dev/null \
        | awk '$2 == "part" { print $1; exit }'
}
