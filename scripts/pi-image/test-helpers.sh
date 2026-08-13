#!/usr/bin/env bash
# Exercise the pure derivations inside the image's boot-time helpers, on a
# dev box: no Pi, no root, no image, no loop mounts. Both scripts expose
# fixture modes for exactly this; before this harness existed they were only
# ever run by hand, which is how a derivation rots between releases.
#
# Run via `just check`, or directly: scripts/pi-image/test-helpers.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly HERE
readonly AP="$HERE/ap-prepare.sh"
readonly USB="$HERE/usb-mount.sh"
readonly FMT="$HERE/format-drive.sh"

pass=0
fail=0

check() {
    local name="$1" want="$2" got="$3"
    if [ "$want" = "$got" ]; then
        pass=$((pass + 1))
    else
        fail=$((fail + 1))
        printf 'FAIL %s\n  want: %s\n  got:  %s\n' "$name" "$want" "$got" >&2
    fi
}

check_fails() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        fail=$((fail + 1))
        printf 'FAIL %s\n  expected a non-zero exit\n' "$name" >&2
    else
        pass=$((pass + 1))
    fi
}

TMP="$(mktemp -d)"
readonly TMP
trap 'rm -rf "$TMP"' EXIT

# ---------------------------------------------------------------- usb-mount

opts() { "$USB" --print-options "$@"; }

# The whole point of the FAT/exFAT branch: no POSIX ownership on the
# filesystem means ownership has to arrive as a mount option, or the daemon
# cannot write and the console reports a read-only drive.
readonly COMMON="noatime,nosuid,nodev,noexec"
for fs in exfat ntfs3; do
    check "options/$fs" \
        "$COMMON,uid=999,gid=985,fmask=0113,dmask=0002" \
        "$(opts "$fs" 999 985)"
done
# The kernel's default vfat iocharset on the appliance is ascii, which
# would mangle any non-ASCII take or project name tribd writes.
check "options/vfat-utf8" \
    "$COMMON,uid=999,gid=985,fmask=0113,dmask=0002,utf8=1" \
    "$(opts vfat 999 985)"

# ext4 carries real ownership; we mount it plain and let a root-owned stick
# report itself unwritable rather than chowning somebody's disk. Passing
# uid= here does not get ignored — the mount fails outright.
check "options/ext4" "$COMMON" "$(opts ext4 999 985)"
check "options/unknown" "$COMMON" "$(opts weirdfs 999 985)"
case "$(opts ext4 999 985)" in
*uid=*)
    fail=$((fail + 1))
    printf 'FAIL options/ext4 must not carry uid= — that mount fails\n' >&2
    ;;
*) pass=$((pass + 1)) ;;
esac

# blkid says "ntfs"; `mount -t ntfs` would route to the ntfs-3g FUSE
# helper, whose ownership semantics differ from the in-kernel driver's.
check "type/ntfs-to-ntfs3" "ntfs3" "$("$USB" --print-type ntfs)"
check "type/exfat-passthrough" "exfat" "$("$USB" --print-type exfat)"
check "type/ext4-passthrough" "ext4" "$("$USB" --print-type ext4)"

# Sustained-write regression guard: `sync` here would quietly ruin
# multitrack recording to USB.
for fs in vfat exfat ntfs3 ext4; do
    case "$(opts "$fs" 999 985)" in
    *sync* | *flush*)
        fail=$((fail + 1))
        printf 'FAIL options/%s must not mount sync\n' "$fs" >&2
        ;;
    *) pass=$((pass + 1)) ;;
    esac
done

mp() { MOUNT_ROOT="$TMP" MOUNTPOINT_BIN=/bin/false "$USB" --print-mountpoint "$@"; }

check "label/plain" "$TMP/RECORDINGS" "$(mp RECORDINGS sda1)"
check "label/spaces" "$TMP/MY_STICK" "$(mp "MY STICK" sda1)"
# Traversal is neutralised twice over: separators fold to underscore, then
# the leading-dot trim removes what is left, so no label can climb out of
# the mount root.
check "label/path-escape" "$TMP/etc" "$(mp "../../etc" sda1)"
check "label/dotdot-only" "$TMP/sda1" "$(mp ".." sda1)"
check "label/slashes" "$TMP/a_b" "$(mp "a/b" sda1)"
check "label/empty-falls-back" "$TMP/sda1" "$(mp "" sda1)"
check "label/junk-only-falls-back" "$TMP/sdb2" "$(mp "///" sdb2)"
check "label/trims-edges" "$TMP/DISK" "$(mp "__DISK__" sda1)"
check "label/collapses-runs" "$TMP/A_B" "$(mp "A***B" sda1)"
check "label/unicode-folded" "$TMP/caf" "$(mp "café" sda1)"
check "label/truncated-to-32" 32 "$(mp "$(printf 'x%.0s' {1..80})" sda1 | sed "s|$TMP/||" | wc -c | tr -d ' ')"

# Collision: a directory with anything in it is a live mount or someone's
# data, so the next insertion must land beside it, not on top of it.
mkdir -p "$TMP/TAKEN" && touch "$TMP/TAKEN/a-file"
check "collision/suffixes" "$TMP/TAKEN-2" "$(mp TAKEN sda1)"
mkdir -p "$TMP/TAKEN-2" && touch "$TMP/TAKEN-2/a-file"
check "collision/walks-on" "$TMP/TAKEN-3" "$(mp TAKEN sda1)"
# An empty leftover from a previous insertion is ours to reuse.
mkdir -p "$TMP/EMPTY"
check "collision/reuses-empty" "$TMP/EMPTY" "$(mp EMPTY sda1)"

check_fails "usb/requires-args" "$USB"

# -------------------------------------------------------------- format-drive

lbl() { "$FMT" --print-label-check "$1"; }

# 11 characters is exFAT's volume-label limit — a filesystem fact. The
# label becomes the volume name, then the mount basename, then the tile,
# so validating to ASCII here makes usb-mount.sh's sanitiser a no-op on
# drives the appliance formatted itself.
check "label/default" ok "$(lbl TRIBUTARY)"
check "label/punctuation" ok "$(lbl FIELD_REC-1)"
check "label/eleven-chars" ok "$(lbl 12345678901)"
check "label/twelve-chars" refused "$(lbl 123456789012)"
check "label/empty" refused "$(lbl '')"
check "label/space" refused "$(lbl 'has space')"
check "label/slash" refused "$(lbl 'a/b')"
check "label/unicode" refused "$(lbl 'café')"
check_fails "format/requires-a-verb" "$FMT"
check_fails "format/rejects-unknown-verb" "$FMT" wipe /dev/sda X

# --------------------------------------------------------------- ap-prepare

ssid() { CPUINFO="$1" MACHINE_ID="$2" "$AP" --print-ssid; }

printf 'Revision\t: c03111\nSerial\t\t: 100000001a2b3c4d\n' > "$TMP/cpuinfo"
printf 'deadbeefcafe0000feedfacedeadbe99\n' > "$TMP/machine-id"
: > "$TMP/empty"

# Serial wins: it survives a reflash, which machine-id does not.
check "ssid/from-serial" "Tributary-3C4D" "$(ssid "$TMP/cpuinfo" "$TMP/machine-id")"
check "ssid/uppercased" "Tributary-3C4D" "$(ssid "$TMP/cpuinfo" "$TMP/missing")"
check "ssid/machine-id-fallback" "Tributary-BE99" "$(ssid "$TMP/empty" "$TMP/machine-id")"
check_fails "ssid/neither-source" ssid "$TMP/empty" "$TMP/missing"

# ------------------------------------------------------------------ summary

printf '%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
