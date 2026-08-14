#!/usr/bin/env bash
# Remaster the official Raspberry Pi OS Lite arm64 image into the Tributary
# appliance image: PipeWire audio stack, a lingering `tributary` service
# user running tribd (LAN-open on port 80), a built-in Wi-Fi access point,
# hostname `tributary`, and the first-boot username wizard masked so a plain
# flash boots straight to serving. The base stays byte-identical everywhere
# else, so stock behavior — first-boot rootfs expansion, Raspberry Pi Imager
# / cloud-init customization, avahi mDNS — is preserved by construction:
# cmdline.txt, the initramfs, and the boot partition's cloud-init files are
# never touched (and verify() proves it).
#
# The access point needs no extra packages: NetworkManager, dnsmasq-base,
# wpasupplicant and the regulatory database all ship in the base. It does
# claim wlan0 outright, so Imager's Wi-Fi credentials no longer apply —
# joining an existing network is ethernet-only (README).
#
# Usage: sudo scripts/pi-image/build.sh <aarch64-tribd> <out.img.xz> [--no-compress]
#   --no-compress  emit the raw .img and skip the xz + Imager JSON (iteration)
# Env:
#   IMAGE_URL_BASE  release download prefix baked into the Imager JSON
#                   (CI passes https://github.com/<repo>/releases/download/<tag>)
#   CACHE_DIR       where the base .img.xz download is kept (default ~/.cache)
#   DEV_SSH_KEY     path to a PUBLIC key — builds a DEV image instead: adds an
#                   SSH login account, key-only sshd, audio debugging tools and
#                   the trib-dev helper. Never set for a release build; the
#                   appliance ships no login account by design.
#   DEV_USER        the dev account's name (default: dev). Never UID 1000 —
#                   that is `pi`, the rename target Imager customization needs.
#   DEV_PASSWORD    optional console-login password for DEV_USER. sshd still
#                   refuses password auth, so this only unlocks the physical
#                   console — the fallback for when SSH itself is the fault.
#
# Runs natively on aarch64 (the release workflow's arm runner — no qemu);
# on an x86_64 box install qemu-user-static and the chroot works
# transparently via binfmt. Local rehearsal: `just pi-image`.
set -euo pipefail

# --- Base image pin: the single home. To bump: pick the newest dated dir at
# https://downloads.raspberrypi.com/raspios_lite_arm64/images/, update BOTH
# lines (sha256 from the published .img.xz.sha256 sidecar), then rehearse
# with `just pi-image` before tagging a release.
readonly BASE_URL="https://downloads.raspberrypi.com/raspios_lite_arm64/images/raspios_lite_arm64-2026-06-19/2026-06-18-raspios-trixie-arm64-lite.img.xz"
# Boards the flasher offers this image. Pi 4 is the floor because the
# real-time path lives or dies on USB jitter, and a Pi 4 puts its xHCI on
# its own PCIe lane where a Pi 3 or Zero 2 W shares one USB2 hub with
# ethernet. (The Zero 2 W was never in this list; the README used to claim
# it anyway.)
readonly IMAGER_DEVICES='["pi4-64bit", "pi5-64bit"]'
readonly BASE_SHA256="acff736ca7945e3b305f07cda4abdb870910e12634991da69783611756e381b3"

# The appliance identity and audio stack. dbus-user-session is named
# explicitly so wireplumber's user D-Bus is never in doubt; pipewire-alsa
# too, because nothing else pulls it in (pipewire-audio is a Depends-only
# metapackage) and without it ALSA's `default` bypasses PipeWire — cpal
# would open raw hardware wireplumber already owns and tribd would boot
# with no usable monitor output. Recommends stay ON so rtkit arrives.
readonly PACKAGES="pipewire pipewire-pulse pipewire-alsa wireplumber pulseaudio-utils dbus-user-session"
readonly TRIB_USER="tributary"
readonly TRIB_HOSTNAME="tributary"
readonly GROW_MIB=768 # deterministic apt headroom; zeros are ~free under xz

# The Wi-Fi regulatory domain the access point runs under. A legal
# constraint that varies by market, so it is a build input for regional
# images rather than a runtime knob. The rest of the AP's settings (SSID
# prefix, passphrase, address, channel) live in tributary-ap.nmconnection.
readonly AP_COUNTRY="${AP_COUNTRY:-US}"
# Mirrors AP_SSID_PREFIX in ap-prepare.sh, which stamps the per-device
# suffix at boot; verify() asserts the two never drift apart.
readonly AP_SSID_PREFIX="Tributary"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly HERE

# --- The release image's sudoers promise: "this image grants no sudo beyond
# the format helper". A pure function over a directory so a dev box can
# exercise it (test-helpers.sh) with no root, no image and no 15-minute
# build — the gap that let a broken version of this check reach a tag.
# Echoes `ok`, or the first offending file and line.
#
# Content, not filenames. The base ships Defaults-only drop-ins (env_keep,
# timestamp_type) whose names drift with every base bump, and a `Defaults`
# line grants nobody anything. What a release must never carry is a user
# spec — precisely the shape a stray DEV_SSH_KEY leaves behind.
# 020_tributary-format is the one deliberate grant, and verify() matches its
# contents exactly, so it is skipped here rather than re-checked loosely.
sudoers_grants() {
    local f spec
    # A path this function never read must not answer "no grants" — that is
    # the same silent-green failure the filename allowlist had.
    if [ ! -d "$1" ]; then
        echo "$1: not a directory"
        return 0
    fi
    for f in "$1"/*; do
        [ -e "$f" ] || continue
        case "${f##*/}" in 020_tributary-format) continue ;; esac
        # `#include`/`@include` are directives, not comments: they pull in a
        # file this loop never sees, so they can hide a grant entirely.
        if grep -qE '^[[:space:]]*[#@]include' "$f"; then
            echo "${f##*/}: include directive"
            return 0
        fi
        spec="$(grep -vE '^[[:space:]]*(#|$)' "$f" \
            | grep -vE '^[[:space:]]*Defaults' | head -1 || true)"
        if [ -n "$spec" ]; then
            echo "${f##*/}: $spec"
            return 0
        fi
    done
    echo ok
}
# Fixture mode, ahead of the usage check below so it needs no build args.
if [ "${1:-}" = --print-sudoers-check ]; then
    sudoers_grants "${2:?usage: build.sh --print-sudoers-check <dir>}"
    exit 0
fi

readonly BINARY="${1:?usage: build.sh <aarch64-tribd> <out.img.xz> [--no-compress]}"
readonly OUT="${2:?usage: build.sh <aarch64-tribd> <out.img.xz> [--no-compress]}"
if [ "${3:-}" = "--no-compress" ]; then readonly COMPRESS=no; else readonly COMPRESS=yes; fi
readonly IMAGE_URL_BASE="${IMAGE_URL_BASE:-http://localhost/UNPUBLISHED}"
readonly CACHE_DIR="${CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/tributary-pi-base}"
# Dev mode is opt-in and leaves no trace when unset: a release image must
# never grow a login account because someone exported a stray variable.
readonly DEV_SSH_KEY="${DEV_SSH_KEY:-}"
readonly DEV_USER="${DEV_USER:-dev}"
# Tools worth having when the question is "what does the audio stack
# actually see" — alsa-utils for arecord -l, pipewire-bin for pw-top and
# pw-dump. Dev images only; the appliance stays lean.
readonly DEV_PACKAGES="openssh-server alsa-utils pipewire-bin"

fail() { echo "build.sh: $*" >&2; exit 1; }

# losetup -P can return before the partition nodes exist (udev race on CI
# hosts; containerized rehearsals have no udev at all) — nudge with partx
# and wait for the rootfs partition to materialize.
wait_parts() {
    for _ in $(seq 20); do
        if [ -b "${LOOP}p2" ]; then return 0; fi
        partx -a "$LOOP" 2>/dev/null || true
        sleep 0.5
    done
    fail "partition nodes for $LOOP never appeared"
}

[ "$(id -u)" = 0 ] || fail "needs root (loop devices, chroot) — run via sudo"
for tool in curl xz sfdisk losetup partx e2fsck resize2fs sha256sum file chroot; do
    command -v "$tool" >/dev/null || fail "missing tool: $tool"
done
file "$BINARY" | grep -q 'ELF 64-bit.*aarch64' || fail "$BINARY is not an aarch64 ELF"
mkdir -p "$(dirname "$OUT")" # crash early, not after the 15-minute build
if [ -n "$DEV_SSH_KEY" ]; then
    [ -f "$DEV_SSH_KEY" ] || fail "DEV_SSH_KEY: no such file: $DEV_SSH_KEY"
    # A private key here would bake a secret into a flashable image AND
    # leave sshd rejecting every login. Both are worth refusing loudly.
    grep -qE '^(ssh-(rsa|ed25519|dss)|ecdsa-sha2-)' "$DEV_SSH_KEY" \
        || fail "DEV_SSH_KEY is not an OpenSSH PUBLIC key: $DEV_SSH_KEY"
    [ "$DEV_USER" != pi ] || fail "DEV_USER must not be pi (UID 1000 is Imager's rename target)"
fi
if [ "$(uname -m)" != aarch64 ]; then
    # binfmt with the F (fix-binary) flag makes the chroot transparent.
    grep -qs 'flags:.*F' /proc/sys/fs/binfmt_misc/qemu-aarch64 \
        || fail "non-arm host: install qemu-user-static (binfmt F flag) to chroot"
fi

# --- Workspace + cleanup. The trap unwinds whatever the failure left
# mounted, in reverse mount order.
WORK="$(mktemp -d)"
ROOT="$WORK/root"
LOOP=""
cleanup() {
    set +e
    for m in dev/pts dev proc sys boot/firmware ""; do
        mountpoint -q "$ROOT/$m" 2>/dev/null && umount "$ROOT/$m"
    done
    [ -n "$LOOP" ] && losetup -d "$LOOP" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

# --- Fetch + verify the pinned base, reusing the cache across runs.
mkdir -p "$CACHE_DIR"
BASE_XZ="$CACHE_DIR/$(basename "$BASE_URL")"
if ! echo "$BASE_SHA256  $BASE_XZ" | sha256sum -c --status 2>/dev/null; then
    echo "==> downloading base image"
    curl -fL --retry 3 -o "$BASE_XZ" "$BASE_URL"
    echo "$BASE_SHA256  $BASE_XZ" | sha256sum -c --status \
        || fail "base image sha256 mismatch — pin or mirror is stale"
fi
IMG="$WORK/tributary.img"
echo "==> decompressing base"
xz -dkc "$BASE_XZ" > "$IMG"

# --- Grow the rootfs before mounting: apt needs headroom the Lite image
# doesn't carry. cmdline.txt is never edited, so stock first-boot expansion
# still grows partition 2 to the full SD card afterwards.
truncate -s "+${GROW_MIB}M" "$IMG"
echo ', +' | sfdisk -N 2 --no-reread "$IMG" >/dev/null
LOOP="$(losetup -Pf --show "$IMG")"
wait_parts
e2fsck -pf "${LOOP}p2" >/dev/null || [ $? -le 2 ] || fail "e2fsck refused the rootfs"
resize2fs "${LOOP}p2" 2>/dev/null

# --- Mount and snapshot the pristine state we promise not to disturb.
mkdir -p "$ROOT"
mount "${LOOP}p2" "$ROOT"
mount "${LOOP}p1" "$ROOT/boot/firmware"
CMDLINE_SHA_BEFORE="$(sha256sum "$ROOT/boot/firmware/cmdline.txt" | cut -d' ' -f1)"
CLOUDINIT_BEFORE="$(cd "$ROOT/boot/firmware" && ls user-data meta-data network-config 2>/dev/null || true)"
for m in dev proc sys; do mount --bind "/$m" "$ROOT/$m"; done
mount --bind /dev/pts "$ROOT/dev/pts"
# raspios ships resolv.conf as a file/symlink managed at boot; park it and
# lend the host's so apt can resolve inside the chroot.
mv "$ROOT/etc/resolv.conf" "$WORK/resolv.conf.orig" 2>/dev/null || true
cp /etc/resolv.conf "$ROOT/etc/resolv.conf"

in_chroot() { chroot "$ROOT" /usr/bin/env DEBIAN_FRONTEND=noninteractive bash -ec "$1"; }

echo "==> installing audio stack"
in_chroot "apt-get update -qq && apt-get install -y -qq $PACKAGES && apt-get clean"

echo "==> service user + linger"
in_chroot "useradd --system --create-home --home-dir /home/$TRIB_USER \
    --shell /usr/sbin/nologin --user-group $TRIB_USER && usermod -aG audio $TRIB_USER"
# The linger flag file is all `loginctl enable-linger` creates; logind scans
# the directory at boot, so writing it directly is chroot-safe. Linger is
# what boots the user session (PipeWire + tribd) with no one logged in.
install -d "$ROOT/var/lib/systemd/linger"
touch "$ROOT/var/lib/systemd/linger/$TRIB_USER"

echo "==> tribd + config"
install -m 755 "$BINARY" "$ROOT/usr/local/bin/tribd"
install -m 644 "$HERE/tribd.service" "$ROOT/etc/systemd/user/tribd.service"
# What `systemctl --user enable` would create, made by hand (no user
# manager runs in a chroot).
install -d "$ROOT/home/$TRIB_USER/.config/systemd/user/default.target.wants"
ln -sf /etc/systemd/user/tribd.service \
    "$ROOT/home/$TRIB_USER/.config/systemd/user/default.target.wants/tribd.service"
install -d "$ROOT/home/$TRIB_USER/config" "$ROOT/home/$TRIB_USER/projects"
install -m 644 "$HERE/tribd.toml" "$ROOT/home/$TRIB_USER/config/tribd.toml"
in_chroot "chown -R $TRIB_USER:$TRIB_USER /home/$TRIB_USER"

# Debian's pipewire/wireplumber packages normally arrive user-enabled via
# their postinst (wireplumber under pipewire.service.wants — any wants-link
# counts). Backfill only if a chrooted postinst deferred; verify() asserts
# the result either way.
user_enabled() { # <unit> — a wants-link anywhere in the global user dirs
    compgen -G "$ROOT/usr/lib/systemd/user/*.wants/$1" >/dev/null \
        || compgen -G "$ROOT/etc/systemd/user/*.wants/$1" >/dev/null
}
ensure_user_enabled() { # <unit> <wants-target used when backfilling>
    if ! user_enabled "$1"; then
        install -d "$ROOT/etc/systemd/user/$2.wants"
        ln -sf "/usr/lib/systemd/user/$1" "$ROOT/etc/systemd/user/$2.wants/$1"
    fi
}
ensure_user_enabled pipewire.socket sockets.target
ensure_user_enabled pipewire-pulse.socket sockets.target
ensure_user_enabled wireplumber.service default.target

echo "==> first-boot wizard preempted"
# On a plain flash, userconf-pi's userconfig.service squats on the console
# asking for a username — an appliance must boot straight to serving. Mask
# it: what `systemctl mask userconfig` creates, made by hand. Masking (not
# disabling) is how Raspberry Pi Imager's own cloud-init path defeats the
# already-queued wizard job, so a masked unit is exactly the state that
# path converges to; Imager customization still creates its user first.
ln -sf /dev/null "$ROOT/etc/systemd/system/userconfig.service"
# The base ships getty@tty1 disabled so the wizard can own the console;
# cancel-rename would re-enable it after user setup. Recreate that enable
# by hand or the console stays blank forever.
install -d "$ROOT/etc/systemd/system/getty.target.wants"
ln -sf /usr/lib/systemd/system/getty@.service \
    "$ROOT/etc/systemd/system/getty.target.wants/getty@tty1.service"
# The stock `pi` user stays exactly as shipped: locked, nologin, UID 1000.
# No login account exists on a plain flash (console/SSH access = reflash
# with Imager customization), and Imager's user setup renames whatever
# user holds UID 1000 — deleting or altering pi would break it.

echo "==> hostname $TRIB_HOSTNAME"
echo "$TRIB_HOSTNAME" > "$ROOT/etc/hostname"
if grep -q raspberrypi "$ROOT/etc/hosts"; then
    sed -i "s/\braspberrypi\b/$TRIB_HOSTNAME/g" "$ROOT/etc/hosts"
else
    echo "127.0.1.1	$TRIB_HOSTNAME" >> "$ROOT/etc/hosts"
fi
# cloud-init would clobber /etc/hostname at first boot if the stock
# user-data names one; align it. Imager customization still overrides —
# it rewrites user-data wholesale.
if [ -f "$ROOT/boot/firmware/user-data" ] \
    && grep -qE '^hostname:' "$ROOT/boot/firmware/user-data"; then
    sed -i -E "s/^hostname:.*/hostname: $TRIB_HOSTNAME/" "$ROOT/boot/firmware/user-data"
fi

echo "==> access point + port 80"
# Config only — every binary this needs is already in the base image.
# NetworkManager refuses a world-readable keyfile holding a PSK, hence 0600.
install -m 600 "$HERE/tributary-ap.nmconnection" \
    "$ROOT/etc/NetworkManager/system-connections/tributary-ap.nmconnection"
install -D -m 755 "$HERE/ap-prepare.sh" "$ROOT/usr/local/lib/tributary/ap-prepare.sh"
install -m 644 "$HERE/tributary-ap-prepare.service" \
    "$ROOT/etc/systemd/system/tributary-ap-prepare.service"
# What `systemctl enable` would create, made by hand (same reason as the
# getty enable above: no manager runs in a chroot).
install -d "$ROOT/etc/systemd/system/multi-user.target.wants"
ln -sf /etc/systemd/system/tributary-ap-prepare.service \
    "$ROOT/etc/systemd/system/multi-user.target.wants/tributary-ap-prepare.service"
# Regulatory domain. raspi-config sets this by editing cmdline.txt, which
# this script pins byte-identical to protect first-boot expansion; cfg80211
# is a module here, so modprobe.d is the same knob with a one-file blast
# radius. Without it the radio has no legal channel list and stays down.
echo "options cfg80211 ieee80211_regdom=$AP_COUNTRY" \
    > "$ROOT/etc/modprobe.d/tributary-regdom.conf"
# `method=shared` hands DHCP+DNS to NetworkManager's own dnsmasq; teach it
# the appliance's name so AP clients with weak mDNS (Android) resolve
# tributary.local too. avahi keeps answering everyone else — two paths, and
# the address has exactly one home: the profile installed above.
AP_ADDR="$(sed -n 's|^address1=\([^/]*\)/.*|\1|p' "$HERE/tributary-ap.nmconnection")"
[ -n "$AP_ADDR" ] || fail "no address1 in tributary-ap.nmconnection"
echo "address=/$TRIB_HOSTNAME.local/$AP_ADDR" \
    > "$ROOT/etc/NetworkManager/dnsmasq-shared.d/tributary.conf"
# The base ships WirelessEnabled=false and NetworkManager honours it, so
# the AP would never appear — silently. Same flip raspi-config makes.
sed -i 's/^WirelessEnabled=.*/WirelessEnabled=true/' \
    "$ROOT/var/lib/NetworkManager/NetworkManager.state"
# Port 80, so the console is just http://tributary.local. tribd runs as a
# systemd USER unit (it needs the session's PipeWire socket), and a user
# manager holds no capabilities to grant via AmbientCapabilities — lowering
# the unprivileged port floor is the one-line alternative. 80 and not 0
# leaves ssh's :22 privileged.
echo "net.ipv4.ip_unprivileged_port_start=80" \
    > "$ROOT/etc/sysctl.d/80-tributary.conf"

echo "==> real-time audio tuning"
# NONE of this makes the box faster on its own. It removes the three things
# that stop a correctly-written audio thread from meeting a 5 ms deadline:
# no permission to ask for real-time scheduling, a CPU that idles at 600 MHz
# until it notices, and a USB stack batching 8 ms of audio into one URB.
#
# The daemon still has to ASK. `tribd` probes these limits once at boot and
# reports what it actually got — see `realtime::posture`.

# 1. Permission. Granted to the SERVICE USER, not to @audio: `audio`
#    membership is how this image grants raw ALSA device access, and
#    putting a second, unrelated promise on the same name means neither can
#    be reasoned about. The numbers match pipewire-bin's own drop-in, which
#    is a defensible thing to point at.
cat > "$ROOT/etc/security/limits.d/95-tributary-audio.conf" <<EOF
# Real-time audio for tribd. Without rtprio, sched_setscheduler(SCHED_FIFO)
# returns EPERM and the daemon runs SCHED_OTHER — which it says out loud
# rather than pretending otherwise.
$TRIB_USER  -  rtprio   95
$TRIB_USER  -  nice     -19
$TRIB_USER  -  memlock  4194304
EOF
chmod 644 "$ROOT/etc/security/limits.d/95-tributary-audio.conf"

# 2. The priority LADDER, and the trap it exists for. tribd is a systemd
#    USER unit under this account and PipeWire runs as the SAME user in the
#    SAME session — so the rtprio limit above is necessarily granted to
#    PipeWire too, whose data thread defaults to rt.prio 88. Left alone it
#    would preempt the audio thread at 10 and undo the whole exercise.
#    Lowering PipeWire is the right half to move: on this box it is fenced
#    off the real-time card and is not in the signal path at all.
mkdir -p "$ROOT/etc/pipewire/pipewire.conf.d"
cat > "$ROOT/etc/pipewire/pipewire.conf.d/99-tributary.conf" <<'EOF'
# Ladder: USB IRQ threads (~50) > tribd engine (10) > PipeWire (5).
context.properties = {
    default.clock.rate          = 48000
    # Every rate the console offers. If the graph is pinned at 48k while
    # the engine is configured to 96k, pipewire-pulse inserts a resampler
    # SILENTLY — degrading the capture this box exists to make.
    default.clock.allowed-rates = [ 44100 48000 96000 ]
    default.clock.quantum       = 256
    default.clock.min-quantum   = 128
    default.clock.max-quantum   = 1024
}
context.modules = [
    { name = libpipewire-module-rt
      args = {
          nice.level = -11
          rt.prio = 5
      }
      flags = [ ifexists nofail ] }
]
EOF
chmod 644 "$ROOT/etc/pipewire/pipewire.conf.d/99-tributary.conf"

# 3. The CPU governor. Raspberry Pi OS defaults to `ondemand`, which
#    samples every 10-20 ms; a 256-frame period at 48 kHz is 5.33 ms, so a
#    burst that starts at 600 MHz misses periods while the governor ramps.
#    A few hundred mW is the right trade for a box whose only job is not to
#    click. A script rather than tmpfiles.d because the policy count varies
#    by SoC and a glob write silently no-ops when the path shape changes.
install -m 755 "$HERE/cpu-governor.sh" "$ROOT/usr/local/lib/tributary/cpu-governor.sh"
install -m 644 "$HERE/tributary-performance.service" \
    "$ROOT/etc/systemd/system/tributary-performance.service"
mkdir -p "$ROOT/etc/systemd/system/multi-user.target.wants"
ln -sf ../tributary-performance.service \
    "$ROOT/etc/systemd/system/multi-user.target.wants/tributary-performance.service"

# 4. USB audio: NOTHING to set, and that is a finding rather than an
#    omission. The tuning everyone reaches for is
#    `options snd_usb_audio nrpacks=1`, and `nrpacks` DOES NOT EXIST on this
#    kernel — verified twice: it is absent from
#    /sys/module/snd_usb_audio/parameters on a modern host, and the
#    modparams check below rejected it against the module this image
#    actually ships. Its replacement, `lowlatency`, is already Y by default.
#    Shipping the line anyway would have been pure cargo cult that the
#    kernel silently ignores.
#
#    `implicit_fb` and `quirk_flags` are per-device, and guessing one breaks
#    hardware nobody here can test. So: measure the xrun rate on the box
#    first (the daemon reports it), and only then reach for a knob.
#
#    The check stays wired regardless, so the moment anyone DOES add
#    /etc/modprobe.d/tributary-usb-audio.conf it is guarded from birth.

# 5. USB autosuspend off for audio interfaces. `usbcore` is built into the
#    Pi kernel, so modprobe.d cannot reach `usbcore.autosuspend` — a udev
#    rule is the only build-time knob. Its own file rather than an addition
#    to 99-tributary-usb.rules: that one is about block devices and mounting,
#    and a second unrelated promise inside it widens its blast radius.
cat > "$ROOT/etc/udev/rules.d/98-tributary-usb-audio.rules" <<'EOF'
# An interface that autosuspends between takes glitches on the first buffer
# after it resumes. 0101 is the USB audio class.
ACTION=="add", SUBSYSTEM=="usb", ENV{ID_USB_INTERFACES}=="*:0101??:*", \
  ATTR{power/control}="on"
EOF
chmod 644 "$ROOT/etc/udev/rules.d/98-tributary-usb-audio.rules"

# 6. Swap stays enabled — this box has no login account and a hard OOM is
#    worse than a slow page. Leaning away from it is free and reversible.
echo "vm.swappiness=10" >> "$ROOT/etc/sysctl.d/80-tributary.conf"
#
# Deliberate NON-changes, recorded so nobody "improves" them later:
#   * kernel.sched_rt_runtime_us stays at 950000. It is the throttle that
#     stops a runaway RT thread wedging a box you cannot ssh into.
#   * No isolcpus and no threadirqs: both are kernel command line, and
#     cmdline.txt is sha-pinned here to protect the first-boot resize token.
#     threadirqs is also unproven for USB audio at a >=5 ms period — measure
#     the xrun rate first, and only then spend that risk.
#   * No PREEMPT_RT kernel: this image stays on the pinned stock base.

echo "==> USB automount"
# Raspberry Pi OS Lite ships no automounter. udisks2 is in the base image
# and runs, but it only mounts when a client calls Filesystem.Mount and a
# headless install has none — so without this a plugged-in stick reaches
# the kernel and stops there, invisible to tribd's mount-table scan.
# Config only: every binary this needs (systemd-mount, findmnt, mountpoint)
# is already in the base, and exfat/ntfs3 ship as kernel modules.
install -D -m 755 "$HERE/usb-mount.sh" "$ROOT/usr/local/lib/tributary/usb-mount.sh"
install -D -m 644 "$HERE/99-tributary-usb.rules" \
    "$ROOT/etc/udev/rules.d/99-tributary-usb.rules"
# The console's Format button. tribd is a systemd USER unit under a nologin
# account with no capabilities, so partitioning needs one root-owned script
# and a sudoers line naming it. 0440 and no dot in the filename: sudo
# ignores a mode-permissive drop-in, and skips any name containing a '.'.
install -D -m 755 "$HERE/format-drive.sh" "$ROOT/usr/local/lib/tributary/format-drive.sh"
install -D -m 440 "$HERE/tributary-format.sudoers" \
    "$ROOT/etc/sudoers.d/020_tributary-format"

if [ -n "$DEV_SSH_KEY" ]; then
    echo "==> DEV image: ssh login + debugging tools"
    in_chroot "apt-get install -y -qq $DEV_PACKAGES && apt-get clean"
    # A real login account, distinct from the nologin service user and from
    # the locked `pi` at UID 1000. Ethernet is the SSH path: the AP profile
    # still claims wlan0, exactly as on the appliance.
    in_chroot "useradd --create-home --shell /bin/bash --user-group $DEV_USER \
        && usermod -aG sudo,audio $DEV_USER"
    # Optional console fallback. Off by default (key-only was the explicit
    # choice), but when SSH is what breaks, a keyboard on the HDMI port is
    # the difference between a diagnosis and a reflash.
    if [ -n "${DEV_PASSWORD:-}" ]; then
        in_chroot "echo '$DEV_USER:$DEV_PASSWORD' | chpasswd"
    fi
    # Key-only over the network regardless: no password auth is accepted by
    # sshd, so a console password never widens the network surface.
    install -d -m 700 "$ROOT/home/$DEV_USER/.ssh"
    install -m 600 "$DEV_SSH_KEY" "$ROOT/home/$DEV_USER/.ssh/authorized_keys"
    in_chroot "chown -R $DEV_USER:$DEV_USER /home/$DEV_USER/.ssh"
    # Passwordless sudo, because every useful question on this image
    # (journal, service session, pactl) needs it and there is no password.
    echo "$DEV_USER ALL=(ALL) NOPASSWD:ALL" > "$ROOT/etc/sudoers.d/010_$DEV_USER-nopasswd"
    chmod 440 "$ROOT/etc/sudoers.d/010_$DEV_USER-nopasswd"
    cat > "$ROOT/etc/ssh/sshd_config.d/10-tributary-dev.conf" <<'EOF'
# Dev image: keys only. The account carries no password, so password and
# keyboard-interactive auth could only ever succeed by accident.
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
EOF
    # ssh.SERVICE, deliberately, not the socket. The base ships no host keys
    # (raspios generates them on first boot) and BOTH generator units —
    # regenerate_ssh_host_keys.service and sshd-keygen.service — declare
    # `Before=ssh.service`. Neither is ordered before ssh.socket. Socket
    # activation therefore races key generation: the first connection spawns
    # an sshd with no host keys, it dies, and systemd stops the rate-limited
    # socket for the rest of the boot — presenting as "connection reset",
    # then "connection refused" forever after.
    in_chroot "systemctl disable ssh.socket >/dev/null 2>&1 || true"
    in_chroot "systemctl enable ssh.service"
    # Generate the host keys HERE rather than trusting first boot. Both
    # generators raspios ships (regenerate_ssh_host_keys, sshd-keygen) are
    # gated on ConditionFirstBoot=yes, so if anything prevents them running
    # on boot one, they never run again and sshd dies on every connection
    # with no host key — unrecoverable without reflashing. A dev image is
    # not a distributed artifact, so a baked key is the right trade; the
    # release path asserts it has none.
    in_chroot "ssh-keygen -A"
    install -m 755 "$HERE/trib-dev" "$ROOT/usr/local/bin/trib-dev"
fi

echo "==> hygiene"
in_chroot "rm -rf /var/lib/apt/lists/*"
truncate -s 0 "$ROOT/etc/machine-id"
rm -f "$ROOT/etc/resolv.conf"
[ -e "$WORK/resolv.conf.orig" ] && mv "$WORK/resolv.conf.orig" "$ROOT/etc/resolv.conf"

for m in dev/pts dev proc sys boot/firmware ""; do umount "$ROOT/$m"; done
losetup -d "$LOOP"; LOOP=""

# --- Verify against a FRESH read-only mount of the finished image — catches
# wrong-mount and ordering bugs the build pass itself can't see.
echo "==> verifying image"
LOOP="$(losetup -Pfr --show "$IMG")"
wait_parts
mount -o ro "${LOOP}p2" "$ROOT"
mount -o ro "${LOOP}p1" "$ROOT/boot/firmware"
v() { "$@" || fail "verify: $*"; }
v test -x "$ROOT/usr/local/bin/tribd"
file "$ROOT/usr/local/bin/tribd" | grep -q 'ELF 64-bit.*aarch64' || fail "verify: binary arch"
v test -f "$ROOT/etc/systemd/user/tribd.service"
v test -L "$ROOT/home/$TRIB_USER/.config/systemd/user/default.target.wants/tribd.service"
v test -f "$ROOT/var/lib/systemd/linger/$TRIB_USER"
v grep -q "^$TRIB_USER:" "$ROOT/etc/passwd"
v grep -qE "^audio:.*[:,]$TRIB_USER(,|\$)" "$ROOT/etc/group"
v grep -q '^bind = "0.0.0.0:80"$' "$ROOT/home/$TRIB_USER/config/tribd.toml"
v test -d "$ROOT/home/$TRIB_USER/projects"
for unit in pipewire.socket pipewire-pulse.socket wireplumber.service; do
    user_enabled "$unit" || fail "verify: $unit not user-enabled"
done
for pkg in pipewire pipewire-pulse pipewire-alsa wireplumber pulseaudio-utils avahi-daemon \
    network-manager dnsmasq-base wpasupplicant; do
    # Status is always the line after Package; a bare Package: stanza also
    # matches half-removed states, so assert the installed one.
    grep -A1 "^Package: $pkg$" "$ROOT/var/lib/dpkg/status" \
        | grep -q '^Status: install ok installed' || fail "verify: $pkg not installed"
done
# The capture stack tribd shells out to, and the ALSA→PipeWire routing that
# keeps cpal's monitor output off raw hardware. parec is a pacat symlink;
# conf.d entries symlink absolutely, so assert link + shipped target.
v test -x "$ROOT/usr/bin/pactl"
v test -x "$ROOT/usr/bin/pacat"
v test -e "$ROOT/usr/bin/parec"
[ -e "$ROOT/etc/alsa/conf.d/99-pipewire-default.conf" ] \
    || [ -L "$ROOT/etc/alsa/conf.d/99-pipewire-default.conf" ] \
    || fail "verify: ALSA default not routed to PipeWire"
v test -f "$ROOT/usr/share/alsa/alsa.conf.d/99-pipewire-default.conf"
[ "$(readlink "$ROOT/etc/systemd/system/userconfig.service")" = /dev/null ] \
    || fail "verify: userconfig.service not masked — first boot would prompt for a username"
v test -L "$ROOT/etc/systemd/system/getty.target.wants/getty@tty1.service"
# The access point. NetworkManager ignores a keyfile whose mode lets anyone
# read the PSK, so the mode is as load-bearing as the content.
AP_PROFILE="$ROOT/etc/NetworkManager/system-connections/tributary-ap.nmconnection"
v test -f "$AP_PROFILE"
[ "$(stat -c%a "$AP_PROFILE")" = 600 ] \
    || fail "verify: AP profile is not mode 600 — NetworkManager would ignore it"
v grep -q '^mode=ap' "$AP_PROFILE"
v grep -q '^method=shared' "$AP_PROFILE"
v grep -q '^psk=' "$AP_PROFILE"
v grep -q "^ssid=$AP_SSID_PREFIX\$" "$AP_PROFILE"
v test -x "$ROOT/usr/local/lib/tributary/ap-prepare.sh"
# The boot-time suffix stamp only lands on the shipped SSID if both sides
# spell the prefix the same way.
v grep -q "^readonly AP_SSID_PREFIX=\"$AP_SSID_PREFIX\"" \
    "$ROOT/usr/local/lib/tributary/ap-prepare.sh"
v test -f "$ROOT/etc/systemd/system/tributary-ap-prepare.service"
v test -L "$ROOT/etc/systemd/system/multi-user.target.wants/tributary-ap-prepare.service"
v grep -q "^address=/$TRIB_HOSTNAME.local/$AP_ADDR\$" \
    "$ROOT/etc/NetworkManager/dnsmasq-shared.d/tributary.conf"
v grep -q "ieee80211_regdom=$AP_COUNTRY" "$ROOT/etc/modprobe.d/tributary-regdom.conf"
# Left false by the base, this alone would keep the radio down with no
# error anywhere on the device.
v grep -q '^WirelessEnabled=true' "$ROOT/var/lib/NetworkManager/NetworkManager.state"
v grep -q '^net.ipv4.ip_unprivileged_port_start=80' "$ROOT/etc/sysctl.d/80-tributary.conf"
# USB automount. Without the rule a stick never reaches the mount table and
# the console's destination list stays blind to it — silently, which is the
# failure this whole path exists to prevent.
# --- real-time tuning
LIMITS="$ROOT/etc/security/limits.d/95-tributary-audio.conf"
v test -f "$LIMITS"
v grep -qE "^${TRIB_USER}[[:space:]]+-[[:space:]]+rtprio[[:space:]]+95\$" "$LIMITS"
v grep -qE "^${TRIB_USER}[[:space:]]+-[[:space:]]+memlock" "$LIMITS"
# The MECHANISM, not the file. limits.d only reaches a lingering user
# manager if pam_limits is in systemd-user's stack; a base bump that dropped
# it would make every line above silently inert and nothing else here would
# notice.
PAM_VERDICT="$(sh "$HERE/pam-limits.sh" --print-limits-check "$ROOT")"
[ "$PAM_VERDICT" = ok ] \
    || fail "verify: rtprio limits would not reach the service session — $PAM_VERDICT"

# The priority ladder. tribd asks for 10 and PipeWire defaults to 88 under
# the SAME user in the SAME session — without this drop-in the rtprio grant
# above hands PipeWire the power to preempt the audio thread.
PW_CONF="$ROOT/etc/pipewire/pipewire.conf.d/99-tributary.conf"
v test -f "$PW_CONF"
v grep -qE '^[[:space:]]*rt\.prio[[:space:]]*=[[:space:]]*5$' "$PW_CONF"
v grep -q 'allowed-rates' "$PW_CONF"

v test -x "$ROOT/usr/local/lib/tributary/cpu-governor.sh"
v test -f "$ROOT/etc/systemd/system/tributary-performance.service"
v test -L "$ROOT/etc/systemd/system/multi-user.target.wants/tributary-performance.service"

# The kernel ignores a mistyped module parameter in SILENCE, so assert that
# every parameter we set is one the shipped module actually accepts.
SND_USB_KO="$(compgen -G "$ROOT/lib/modules/*/kernel/sound/usb/snd-usb-audio.ko*" | head -1 || true)"
MODPARAM_VERDICT="$(sh "$HERE/modparams.sh" --check "${SND_USB_KO:-/nonexistent}" \
    "$ROOT/etc/modprobe.d/tributary-usb-audio.conf")"
[ "$MODPARAM_VERDICT" = ok ] || fail "verify: $MODPARAM_VERDICT"

v grep -q 'ATTR{power/control}="on"' "$ROOT/etc/udev/rules.d/98-tributary-usb-audio.rules"
v grep -q '^vm.swappiness=10$' "$ROOT/etc/sysctl.d/80-tributary.conf"
# The appliance takes its card outright; the shared layer would leave it at
# the desktop's latency with nobody able to tell.
v grep -q '^layer = "exclusive"$' "$ROOT/home/$TRIB_USER/config/tribd.toml"
USB_RULE="$ROOT/etc/udev/rules.d/99-tributary-usb.rules"
v test -f "$USB_RULE"
v test -x "$ROOT/usr/local/lib/tributary/usb-mount.sh"
# ID_BUS is what keeps the boot media out: the SD card carries no ID_BUS at
# all, so losing this line would put / on the destination list.
v grep -q 'ENV{ID_BUS}!="usb"' "$USB_RULE"
# The rule names the helper by absolute path; a rename on one side only
# would fail silently at hotplug time, which nothing else here would catch.
v grep -q 'RUN+="/usr/local/lib/tributary/usb-mount.sh' "$USB_RULE"
# `change` is how a console-formatted drive remounts without a replug.
v grep -q 'ACTION!="add|change"' "$USB_RULE"
# Behaviour of the SHIPPED helper, not the repo copy — the scripts are
# arch-independent text, so the build host can run the aarch64 image's own
# file and assert the strings the kernel would actually receive.
USB_HELPER="$ROOT/usr/local/lib/tributary/usb-mount.sh"
USB_OPTS="$(bash "$USB_HELPER" --print-options vfat 999 985)"
case ",$USB_OPTS," in
*,sync,* | *,flush,*) fail "verify: mount options include sync/flush — a take is a sustained write" ;;
esac
case "$USB_OPTS" in
*uid=999,gid=985*) ;;
*) fail "verify: vfat carries no uid=/gid= — the daemon could not write: $USB_OPTS" ;;
esac
case "$(bash "$USB_HELPER" --print-options ext4 999 985)" in
*uid=*) fail "verify: uid= passed to ext4 — that mount fails outright" ;;
esac
[ "$(bash "$USB_HELPER" --print-type ntfs)" = ntfs3 ] \
    || fail "verify: ntfs would route to the ntfs-3g FUSE helper"
# The format helper's trust boundary is one script plus the sudoers line
# naming it, so ownership and mode are as load-bearing as the contents.
FMT="$ROOT/usr/local/lib/tributary/format-drive.sh"
v test -x "$FMT"
[ "$(stat -c '%u:%g:%a' "$FMT")" = 0:0:755 ] \
    || fail "verify: format-drive.sh must be root-owned and not writable by $TRIB_USER"
[ "$(stat -c '%u:%g:%a' "$ROOT/usr/local/lib/tributary")" = 0:0:755 ] \
    || fail "verify: /usr/local/lib/tributary is not root-owned"
FMT_SUDO="$ROOT/etc/sudoers.d/020_tributary-format"
v test -f "$FMT_SUDO"
[ "$(stat -c%a "$FMT_SUDO")" = 440 ] \
    || fail "verify: sudoers drop-in mode — sudo ignores anything more permissive"
v grep -q "^$TRIB_USER ALL=(root:root) NOPASSWD: /usr/local/lib/tributary/format-drive.sh\$" "$FMT_SUDO"
[ "$(bash "$FMT" --print-label-check TRIBUTARY)" = ok ] || fail "verify: label check"
[ "$(bash "$FMT" --print-label-check 'has space')" = refused ] || fail "verify: label check"
# The filesystems a stick actually arrives formatted as. vfat is builtin;
# these two are modules, and mkfs.exfat is what the format helper needs.
for m in exfat ntfs3; do
    compgen -G "$ROOT/lib/modules/*/kernel/fs/$m/$m.ko*" >/dev/null \
        || fail "verify: no $m kernel module — USB sticks of that type cannot mount"
done
v test -x "$ROOT/sbin/mkfs.exfat"
# pi must stay locked (no accidental credentials) at UID 1000 (the rename
# target Imager customization depends on).
v grep -q '^pi:x:1000:1000:' "$ROOT/etc/passwd"
v grep -q '^pi:!:' "$ROOT/etc/shadow"
if [ -n "$DEV_SSH_KEY" ]; then
    v grep -q "^$DEV_USER:" "$ROOT/etc/passwd"
    v grep -q '^ssh-' "$ROOT/home/$DEV_USER/.ssh/authorized_keys"
    [ "$(stat -c%a "$ROOT/home/$DEV_USER/.ssh")" = 700 ] \
        || fail "verify: ~/.ssh mode — sshd refuses a group/world-readable key dir"
    [ "$(stat -c%a "$ROOT/home/$DEV_USER/.ssh/authorized_keys")" = 600 ] \
        || fail "verify: authorized_keys mode"
    # An account with no password AND no working key would be a brick.
    v grep -q '^PasswordAuthentication no' "$ROOT/etc/ssh/sshd_config.d/10-tributary-dev.conf"
    v test -L "$ROOT/etc/systemd/system/multi-user.target.wants/ssh.service"
    # The socket would race host-key generation on first boot; the service
    # is ordered after it. Assert the racy one stayed off.
    if [ -e "$ROOT/etc/systemd/system/sockets.target.wants/ssh.socket" ]; then
        fail "verify: ssh.socket enabled — it races host-key generation"
    fi
    # Without a host key sshd resets every connection, and raspios' two
    # generators are first-boot-only — so a missing key here is a brick.
    compgen -G "$ROOT/etc/ssh/ssh_host_*_key" >/dev/null \
        || fail "verify: no sshd host keys — every connection would be reset"
    v grep -q "^$DEV_USER ALL=(ALL) NOPASSWD:ALL" "$ROOT/etc/sudoers.d/010_$DEV_USER-nopasswd"
    v grep -qE "^sudo:.*[:,]$DEV_USER(,|\$)" "$ROOT/etc/group"
    v test -x "$ROOT/usr/local/bin/trib-dev"
    for pkg in openssh-server alsa-utils pipewire-bin; do
        grep -A1 "^Package: $pkg$" "$ROOT/var/lib/dpkg/status" \
            | grep -q '^Status: install ok installed' || fail "verify: $pkg not installed"
    done
else
    # A release image must carry no login account at all. This is the
    # assertion that stops a stray DEV_SSH_KEY leaking into a tagged build.
    # `if`, not `[ … ] && fail`: as a statement that trips set -e on the
    # normal (guard-false) path and aborts a perfectly good build.
    if [ -e "$ROOT/etc/ssh/sshd_config.d/10-tributary-dev.conf" ]; then
        fail "verify: dev sshd config in a non-dev image"
    fi
    # Not a glob for the dev rule: a check that only looked for
    # `010_*-nopasswd` would stay green while the promise it encodes quietly
    # stopped being true. sudoers_grants() judges every drop-in by content,
    # so the promise keeps its meaning across a base bump that renames or
    # adds one — an allowlist of filenames would only keep its wording.
    SUDOERS_VERDICT="$(sudoers_grants "$ROOT/etc/sudoers.d")"
    [ "$SUDOERS_VERDICT" = ok ] \
        || fail "verify: sudoers drop-in grants privileges in a release image — $SUDOERS_VERDICT"
    # Baked host keys are fine for a one-off dev image and wrong for a
    # release: every card flashed from it would share an identity.
    if compgen -G "$ROOT/etc/ssh/ssh_host_*_key" >/dev/null; then
        fail "verify: sshd host keys baked into a release image"
    fi
    v test ! -e "$ROOT/usr/local/bin/trib-dev"
fi
[ "$(cat "$ROOT/etc/hostname")" = "$TRIB_HOSTNAME" ] || fail "verify: hostname"
v grep -q "$TRIB_HOSTNAME" "$ROOT/etc/hosts"
[ "$(sha256sum "$ROOT/boot/firmware/cmdline.txt" | cut -d' ' -f1)" = "$CMDLINE_SHA_BEFORE" ] \
    || fail "verify: cmdline.txt changed — first-boot expansion is at risk"
[ "$(cd "$ROOT/boot/firmware" && ls user-data meta-data network-config 2>/dev/null || true)" = "$CLOUDINIT_BEFORE" ] \
    || fail "verify: cloud-init boot files changed — Imager customization is at risk"
if [ -f "$ROOT/boot/firmware/user-data" ] \
    && grep -qE '^hostname: raspberrypi' "$ROOT/boot/firmware/user-data"; then
    fail "verify: stock hostname survived in user-data"
fi
if [ -s "$ROOT/etc/machine-id" ]; then fail "verify: machine-id not zeroed"; fi
if [ -e "$ROOT/usr/bin/qemu-aarch64-static" ]; then fail "verify: stray qemu binary"; fi
umount "$ROOT/boot/firmware" "$ROOT"
losetup -d "$LOOP"; LOOP=""
echo "==> verify OK"

# --- Package. Raw-image size/sha go into the Imager JSON (computed before
# compression), which is what restores Imager's OS-customization dialog for
# a non-official image (init_format: cloudinit-rpi matches the Trixie base).
if [ "$COMPRESS" = no ]; then
    mv "$IMG" "${OUT%.xz}"
    echo "==> ${OUT%.xz} (uncompressed, no Imager JSON)"
    exit 0
fi
EXTRACT_SIZE="$(stat -c%s "$IMG")"
EXTRACT_SHA256="$(sha256sum "$IMG" | cut -d' ' -f1)"
echo "==> compressing"
xz -T0 -c "$IMG" > "$OUT"
sha256sum "$OUT" > "$OUT.sha256"
DOWNLOAD_SIZE="$(stat -c%s "$OUT")"
DOWNLOAD_SHA256="$(sha256sum "$OUT" | cut -d' ' -f1)"
cat > "${OUT%.img.xz}-imager.json" <<EOF
{
  "os_list": [
    {
      "name": "Tributary appliance",
      "description": "Raspberry Pi OS Lite arm64 with the Tributary recording console preinstalled — hosts its own Wi-Fi network and serves the console at http://tributary.local",
      "url": "$IMAGE_URL_BASE/$(basename "$OUT")",
      "release_date": "$(date -u +%F)",
      "image_download_size": $DOWNLOAD_SIZE,
      "image_download_sha256": "$DOWNLOAD_SHA256",
      "extract_size": $EXTRACT_SIZE,
      "extract_sha256": "$EXTRACT_SHA256",
      "devices": $IMAGER_DEVICES,
      "init_format": "cloudinit-rpi"
    }
  ]
}
EOF
echo "==> $OUT"
