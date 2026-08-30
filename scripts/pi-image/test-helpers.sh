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
readonly DRIVE="$HERE/drive-mount.sh"
readonly FMT="$HERE/format-drive.sh"
readonly BUILD="$HERE/build.sh"

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

# -------------------------------------------------------------- drive-mount

opts() { "$DRIVE" --print-options "$@"; }

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
check "type/ntfs-to-ntfs3" "ntfs3" "$("$DRIVE" --print-type ntfs)"
check "type/exfat-passthrough" "exfat" "$("$DRIVE" --print-type exfat)"
check "type/ext4-passthrough" "ext4" "$("$DRIVE" --print-type ext4)"

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

mp() { MOUNT_ROOT="$TMP" MOUNTPOINT_BIN=/bin/false "$DRIVE" --print-mountpoint "$@"; }

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

check_fails "drive/requires-args" "$DRIVE"

# -------------------------------------------------------------- format-drive

lbl() { "$FMT" --print-label-check "$1"; }

# 11 characters is exFAT's volume-label limit — a filesystem fact. The
# label becomes the volume name, then the mount basename, then the tile,
# so validating to ASCII here makes drive-mount.sh's sanitiser a no-op on
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

fs() { "$FMT" --print-fs-check "$1"; }

# Two filesystems and no others. exFAT for a drive that gets unplugged and
# opened on a laptop; ext4 for one that lives in the recorder. Anything
# else is refused rather than passed through to mkfs.
check "fs/exfat" ok "$(fs exfat)"
check "fs/ext4" ok "$(fs ext4)"
check "fs/ntfs-not-offered" refused "$(fs ntfs)"
check "fs/empty" refused "$(fs '')"
check "fs/injection" refused "$(fs 'ext4 /dev/mmcblk0')"

# ------------------------------------------------------- block-device facts

# The guard that replaced ID_BUS=="usb". A fixture sysfs tree and mountinfo
# file are enough to exercise it with no root and no real disk, which is the
# whole reason the helpers take $SYS_BLOCK and $MOUNTINFO from the
# environment.
#
# Layout: an SD card (179:0) with two partitions, a USB stick (8:0) with
# one, and an NVMe SSD (259:0) with one.
readonly SYSB="$TMP/sysblock"
mkdir -p "$SYSB"
mkdev() { # <devno> <path-under-$TMP/devices> [partition]
    local devno="$1" path="$TMP/devices/$2"
    mkdir -p "$path"
    # Newline-terminated, exactly as the kernel writes it.
    printf '%s\n' "$devno" >"$path/dev"
    [ "${3:-}" = partition ] && : >"$path/partition"
    ln -sfn "$path" "$SYSB/$devno"
}
mkdev 179:0 mmcblk0
mkdev 179:1 mmcblk0/mmcblk0p1 partition
mkdev 179:2 mmcblk0/mmcblk0p2 partition
mkdev 8:0 sda
mkdev 8:1 sda/sda1 partition
mkdev 259:0 nvme0n1
mkdev 259:1 nvme0n1/nvme0n1p1 partition

# mountinfo: field 3 is major:minor, field 5 is the mount point.
cat >"$TMP/mountinfo-sd" <<'MI'
26 1 179:2 / / rw,noatime - ext4 /dev/mmcblk0p2 rw
27 26 179:1 / /boot/firmware rw - vfat /dev/mmcblk0p1 rw
40 26 8:1 / /media/STICK rw - exfat /dev/sda1 rw
MI

sysck() { MOUNTINFO="$TMP/mountinfo-$1" SYS_BLOCK="$SYSB" "$FMT" --print-system-check "$2"; }

check "system/sd-boot-disk" system "$(sysck sd 179:0)"
check "system/sd-boot-partition" system "$(sysck sd 179:2)"
check "system/sd-boot-firmware" system "$(sysck sd 179:1)"
check "system/usb-stick-is-not" other "$(sysck sd 8:0)"
check "system/usb-partition-is-not" other "$(sysck sd 8:1)"
# The case the old bus test got wrong in the other direction: an NVMe that
# is merely attached is fair game.
check "system/nvme-attached-is-not" other "$(sysck sd 259:0)"
check "system/unknown-devno" other "$(sysck sd 253:7)"

# Boot from NVMe and the answer flips — which is the point. A bus test
# could never do this: it would have called the same drive formattable.
cat >"$TMP/mountinfo-nvme" <<'MI'
26 1 259:1 / / rw,noatime - ext4 /dev/nvme0n1p1 rw
40 26 8:1 / /media/STICK rw - exfat /dev/sda1 rw
MI
# Root on a whole disk with /boot on one of its partitions: the two
# lookups take different branches of parent_devno, which is precisely the
# shape that used to concatenate into one meaningless devno and quietly
# stop recognising the boot disk.
cat >"$TMP/mountinfo-wholedisk" <<'MI'
26 1 8:0 / / rw,noatime - ext4 /dev/sda rw
27 26 8:1 / /boot rw - vfat /dev/sda1 rw
MI
check "system/mixed-branch-lookup" system "$(sysck wholedisk 8:0)"
check "system/mixed-branch-other-disk" other "$(sysck wholedisk 179:0)"

check "system/nvme-boot-disk" system "$(sysck nvme 259:0)"
check "system/nvme-boot-partition" system "$(sysck nvme 259:1)"
check "system/sd-is-not-when-nvme-booted" other "$(sysck nvme 179:0)"

# Both helpers must answer identically: they share one implementation, and
# a copy that drifted is exactly what this asserts cannot happen.
check "system/mount-helper-agrees" \
    "$(sysck sd 179:0)" \
    "$(MOUNTINFO="$TMP/mountinfo-sd" SYS_BLOCK="$SYSB" "$DRIVE" --print-system-check 179:0)"
check "system/mount-helper-agrees-negative" \
    "$(sysck sd 8:0)" \
    "$(MOUNTINFO="$TMP/mountinfo-sd" SYS_BLOCK="$SYSB" "$DRIVE" --print-system-check 8:0)"

# The partition node is read back from the kernel rather than spelled:
# ${DEV}1 is sda1 but nvme0n11, which is nothing.
cat >"$TMP/fake-lsblk" <<'LSB'
#!/usr/bin/env bash
case "${!#}" in
/dev/sda) printf '/dev/sda disk\n/dev/sda1 part\n' ;;
/dev/nvme0n1) printf '/dev/nvme0n1 disk\n/dev/nvme0n1p1 part\n' ;;
/dev/mmcblk0) printf '/dev/mmcblk0 disk\n/dev/mmcblk0p1 part\n/dev/mmcblk0p2 part\n' ;;
esac
LSB
chmod +x "$TMP/fake-lsblk"
partnode() { LSBLK="$TMP/fake-lsblk" "$FMT" --print-part-node "$1"; }

check "part-node/sata" /dev/sda1 "$(partnode /dev/sda)"
check "part-node/nvme" /dev/nvme0n1p1 "$(partnode /dev/nvme0n1)"
check "part-node/mmc-takes-the-first" /dev/mmcblk0p1 "$(partnode /dev/mmcblk0)"
check "part-node/no-partition" "" "$(partnode /dev/sdz)"

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

# ------------------------------------------------- build.sh release sudoers

sud() { "$BUILD" --print-sudoers-check "$1"; }

# The four Defaults-only drop-ins Raspberry Pi OS Lite trixie ships, verbatim.
# They grant nobody anything, so a release image carrying them is still an
# image with no login account — and their names are the base's business, not
# ours. A filename allowlist failed here and blocked a release.
mkdir -p "$TMP/sud-base"
echo 'Defaults env_keep += "NO_AT_BRIDGE"' > "$TMP/sud-base/010_at-export"
echo 'Defaults env_keep += "DPKG_DEB_THREADS_MAX"' > "$TMP/sud-base/010_dpkg-threads"
echo 'Defaults timestamp_type=global' > "$TMP/sud-base/010_global-tty"
echo 'Defaults env_keep += "http_proxy HTTP_PROXY"' > "$TMP/sud-base/010_proxy"
printf '# sudo reads every file here.\n#\n# Comments only.\n' > "$TMP/sud-base/README"
check "sudoers/base-defaults-only" ok "$(sud "$TMP/sud-base")"

# The appliance's own grant is deliberate, and verify() matches its text
# exactly elsewhere — so this loop must not also flag it as a user spec.
mkdir -p "$TMP/sud-fmt"
cp "$HERE/tributary-format.sudoers" "$TMP/sud-fmt/020_tributary-format"
check "sudoers/format-helper-allowed" ok "$(sud "$TMP/sud-fmt")"

# The regression this check exists for: a stray DEV_SSH_KEY leaving a login
# account and passwordless sudo in a tagged release image.
mkdir -p "$TMP/sud-dev"
cp "$TMP/sud-base/010_proxy" "$TMP/sud-dev/010_proxy"
echo 'dev ALL=(ALL) NOPASSWD:ALL' > "$TMP/sud-dev/010_dev-nopasswd"
check "sudoers/dev-rule-caught" \
    "010_dev-nopasswd: dev ALL=(ALL) NOPASSWD:ALL" "$(sud "$TMP/sud-dev")"

# `#include` is a directive, not a comment: it pulls in a file this scan
# never opens, so treating it as a comment would leave a hole the width of
# the whole check.
mkdir -p "$TMP/sud-inc"
printf '# helpful preamble\n@includedir /etc/sudoers.extra\n' > "$TMP/sud-inc/030_extra"
check "sudoers/include-caught" "030_extra: include directive" "$(sud "$TMP/sud-inc")"
mkdir -p "$TMP/sud-inc-hash"
printf '#includedir /etc/sudoers.extra\n' > "$TMP/sud-inc-hash/030_extra"
check "sudoers/hash-include-caught" \
    "030_extra: include directive" "$(sud "$TMP/sud-inc-hash")"

# An empty directory is the no-drop-ins case, not an error.
mkdir -p "$TMP/sud-empty"
check "sudoers/empty-dir" ok "$(sud "$TMP/sud-empty")"

# ...but a path that was never read must not answer "no grants". A mistyped
# path silently passing forever is the failure the allowlist already had.
check "sudoers/missing-dir-is-not-ok" \
    "$TMP/sud-absent: not a directory" "$(sud "$TMP/sud-absent")"

# --------------------------------------------------------- cpu-governor.sh

gov() { sh "$HERE/cpu-governor.sh" "$@"; }

mkdir -p "$TMP/cpufreq/policy0" "$TMP/cpufreq/policy4"
touch "$TMP/cpufreq/policy0/scaling_governor" "$TMP/cpufreq/policy4/scaling_governor"
check "governor/both-policies" \
    "$TMP/cpufreq/policy0/scaling_governor
$TMP/cpufreq/policy4/scaling_governor" \
    "$(gov --print-policies "$TMP/cpufreq")"

# A SoC with no cpufreq at all must not fail the boot of a box that has no
# login account: empty, exit 0.
check "governor/no-cpufreq" "" "$(gov --print-policies "$TMP/cpufreq-absent")"
check "governor/name" performance "$(gov --print-governor)"

# --------------------------------------------------------- modparams.sh

mod() { sh "$HERE/modparams.sh" "$@"; }

# A stand-in module: `strings` finds the same `parm=` entries modinfo reads.
printf 'parm=nrpacks:Max. number of packets per URB (int)\nparm=lowlatency:.\n' \
    > "$TMP/fake-module.ko"
printf 'options snd_usb_audio nrpacks=1\n' > "$TMP/modconf-good"
check "modparams/real-parameter" ok "$(mod --check "$TMP/fake-module.ko" "$TMP/modconf-good")"

printf 'options snd_usb_audio nrpaks=1\n' > "$TMP/modconf-typo"
check "modparams/typo-caught" \
    "fake-module.ko: no such parameter: nrpaks" \
    "$(mod --check "$TMP/fake-module.ko" "$TMP/modconf-typo")"

: > "$TMP/modconf-empty"
check "modparams/empty-conf" ok "$(mod --check "$TMP/fake-module.ko" "$TMP/modconf-empty")"

# A module that could not be read proves NOTHING, so it must not answer ok
# — the exact failure mode the sudoers filename allowlist had.
# Nothing set is nothing to get wrong — but only when the conf is genuinely
# absent, never when the MODULE could not be read.
check "modparams/absent-conf-is-ok" ok "$(mod --check "$TMP/fake-module.ko" "$TMP/no-such-conf")"

check "modparams/missing-module-is-not-ok" \
    "nope.ko: module not found, cannot verify parameters" \
    "$(mod --check "$TMP/nope.ko" "$TMP/modconf-good")"

# --------------------------------------------------------- pam-limits.sh

pam() { sh "$HERE/pam-limits.sh" --print-limits-check "$1"; }

mkdir -p "$TMP/pam-usr/usr/lib/pam.d"
printf 'session required pam_limits.so\n' > "$TMP/pam-usr/usr/lib/pam.d/systemd-user"
check "pam/found-in-usr-lib" ok "$(pam "$TMP/pam-usr")"

mkdir -p "$TMP/pam-etc/etc/pam.d"
printf 'session required pam_limits.so\n' > "$TMP/pam-etc/etc/pam.d/systemd-user"
check "pam/found-in-etc" ok "$(pam "$TMP/pam-etc")"

# Debian factors these through @include; following one level is the
# difference between a real check and a grep that happens to pass.
mkdir -p "$TMP/pam-inc/etc/pam.d"
printf '@include common-session\n' > "$TMP/pam-inc/etc/pam.d/systemd-user"
printf 'session required pam_limits.so\n' > "$TMP/pam-inc/etc/pam.d/common-session"
check "pam/found-via-include" ok "$(pam "$TMP/pam-inc")"

mkdir -p "$TMP/pam-none/etc/pam.d"
printf 'session required pam_unix.so\n' > "$TMP/pam-none/etc/pam.d/systemd-user"
check "pam/no-pam-limits-caught" "systemd-user: no pam_limits" "$(pam "$TMP/pam-none")"

# And a rootfs with no systemd-user at all is a failure, not a pass.
mkdir -p "$TMP/pam-empty"
check "pam/missing-file-is-not-ok" "systemd-user: not found" "$(pam "$TMP/pam-empty")"

# ---------------------------------------------------------- soundfonts

# The manifest parser, against a fixture manifest — no network, no 380 MB.
# It is exercised at all because it already hid a real bug: the key class
# was [a-z_]+, which does not match `sha256`, so every hash parsed as empty
# and every file compared unequal. That surfaced as "corrupt GeneralUser-
# GS.sf2" on a byte-perfect download — a parser fault wearing a data
# fault's clothes.
readonly FETCH="$HERE/../fetch-soundfonts.sh"
cat >"$TMP/manifest.toml" <<'MF'
# A comment that must not parse as a key.
[[soundfont]]
id = "Alpha.sf2"
name = "Alpha Bank"
version = "1.2"
default = true
bytes = 12
sha256 = "aaaa"
url = "https://example.invalid/alpha.sf2"
license = "MIT"
license_url = "https://example.invalid/alpha"
author = "A. Person, with a comma"
description = "first"

[[soundfont]]
id = "Beta.sf2"
name = "Beta Bank"
version = "3.1"
bytes = 34
sha256 = "bbbb"
archive_sha256 = "cccc"
archive_member = "beta-3.1/Beta.sf2"
url = "https://example.invalid/beta.tar.gz"
license = "MIT"
license_url = "https://example.invalid/beta"
author = "B. Person"
description = "second"
MF
sf() { MANIFEST="$TMP/manifest.toml" DEST="$TMP/sfdist" "$FETCH" "$@"; }

check "soundfonts/ids" "Alpha.sf2 Beta.sf2" "$(sf --print-ids | tr '\n' ' ' | sed 's/ $//')"
# Exactly one default, attributed to its own font rather than to whichever
# block was read last.
check "soundfonts/default" "Alpha.sf2" "$(sf --print-default)"

# Values containing spaces and commas must survive whole. awk's default
# whitespace splitting truncated every one of these at the first space.
ATTRIB="$(sf --print-attribution)"
check "soundfonts/attribution-name" \
    "- **Alpha Bank** 1.2 by A. Person, with a comma — MIT. <https://example.invalid/alpha>" \
    "$(printf '%s\n' "$ATTRIB" | grep '^- \*\*Alpha')"
check "soundfonts/attribution-lists-every-font" 2 \
    "$(printf '%s\n' "$ATTRIB" | grep -c '^- \*\*')"
check "soundfonts/attribution-has-a-heading" "## SoundFonts" \
    "$(printf '%s\n' "$ATTRIB" | head -1)"

# --check never downloads, and says which of the two failure shapes it is.
mkdir -p "$TMP/sfdist"
check_fails "soundfonts/check-reports-missing" sf --check
check "soundfonts/missing-names-the-file" 1 \
    "$(sf --check 2>&1 | grep -c 'missing  Alpha.sf2')"
# 12 bytes whose sha256 is not "aaaa": present but wrong is a different
# sentence from absent, because the fix is different.
printf '%s' 'not the font' >"$TMP/sfdist/Alpha.sf2"
check "soundfonts/corrupt-is-not-missing" 1 \
    "$(sf --check 2>&1 | grep -c 'corrupt  Alpha.sf2')"

check_fails "soundfonts/rejects-unknown-mode" sf --nope

# ------------------------------------------------ pipewire rate agreement

# `allowed-rates` and `SAMPLE_RATES` are the same fact in two files. If the
# graph is pinned at 48k while the console offers 96k, pipewire-pulse
# inserts a resampler silently — degrading the capture this box exists to
# make. A repo-local check, needing no image.
RATES_RS="$(sed -n 's/.*SAMPLE_RATES: \[u32; [0-9]*\] = \[\(.*\)\];/\1/p' \
    "$HERE/../../crates/tribd/src/settings.rs" | tr -d ' _' | tr ',' '\n' | sort -n | tr '\n' ' ')"
RATES_CONF="$(sed -n 's/.*allowed-rates[[:space:]]*=[[:space:]]*\[\(.*\)\].*/\1/p' \
    "$HERE/build.sh" | tr -s ' ' '\n' | grep -E '^[0-9]+$' | sort -n | tr '\n' ' ')"
check "pipewire/rates-agree" "$RATES_RS" "$RATES_CONF"

# ------------------------------------------------------------------ summary

printf '%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
