#!/usr/bin/env bash
# Remaster the official Raspberry Pi OS Lite arm64 image into the Tributary
# appliance image: PipeWire audio stack, a lingering `tributary` service
# user running tribd (LAN-open), hostname `tributary`, and the first-boot
# username wizard masked so a plain flash boots straight to serving. The
# base stays byte-identical everywhere else, so stock behavior — first-boot
# rootfs expansion, Raspberry Pi Imager / cloud-init customization, avahi
# mDNS — is preserved by construction: cmdline.txt, the initramfs, and the
# boot partition's cloud-init files are never touched (and verify() proves
# it).
#
# Usage: sudo scripts/pi-image/build.sh <aarch64-tribd> <out.img.xz> [--no-compress]
#   --no-compress  emit the raw .img and skip the xz + Imager JSON (iteration)
# Env:
#   IMAGE_URL_BASE  release download prefix baked into the Imager JSON
#                   (CI passes https://github.com/<repo>/releases/download/<tag>)
#   CACHE_DIR       where the base .img.xz download is kept (default ~/.cache)
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

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly HERE
readonly BINARY="${1:?usage: build.sh <aarch64-tribd> <out.img.xz> [--no-compress]}"
readonly OUT="${2:?usage: build.sh <aarch64-tribd> <out.img.xz> [--no-compress]}"
if [ "${3:-}" = "--no-compress" ]; then readonly COMPRESS=no; else readonly COMPRESS=yes; fi
readonly IMAGE_URL_BASE="${IMAGE_URL_BASE:-http://localhost/UNPUBLISHED}"
readonly CACHE_DIR="${CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/tributary-pi-base}"

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
v grep -q '0.0.0.0:4600' "$ROOT/home/$TRIB_USER/config/tribd.toml"
v test -d "$ROOT/home/$TRIB_USER/projects"
for unit in pipewire.socket pipewire-pulse.socket wireplumber.service; do
    user_enabled "$unit" || fail "verify: $unit not user-enabled"
done
for pkg in pipewire pipewire-pulse pipewire-alsa wireplumber pulseaudio-utils avahi-daemon; do
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
# pi must stay locked (no accidental credentials) at UID 1000 (the rename
# target Imager customization depends on).
v grep -q '^pi:x:1000:1000:' "$ROOT/etc/passwd"
v grep -q '^pi:!:' "$ROOT/etc/shadow"
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
      "description": "Raspberry Pi OS Lite arm64 with the Tributary recording console preinstalled — boots serving http://tributary.local:4600",
      "url": "$IMAGE_URL_BASE/$(basename "$OUT")",
      "release_date": "$(date -u +%F)",
      "image_download_size": $DOWNLOAD_SIZE,
      "image_download_sha256": "$DOWNLOAD_SHA256",
      "extract_size": $EXTRACT_SIZE,
      "extract_sha256": "$EXTRACT_SHA256",
      "devices": ["pi3-64bit", "pi4-64bit", "pi5-64bit"],
      "init_format": "cloudinit-rpi"
    }
  ]
}
EOF
echo "==> $OUT"
