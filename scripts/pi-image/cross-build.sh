#!/usr/bin/env bash
# Cross-build an aarch64 `tribd` (web console embedded) for the Pi, from an
# x86_64 workstation that has neither the Rust target nor an aarch64 linker.
#
# Native x86_64 compilation with an aarch64 cross-linker, inside a container
# — NOT emulation. Building the whole workspace under qemu-user works but
# takes ~15 minutes; this takes about as long as a normal release build,
# which is what makes an edit-deploy-test loop bearable.
#
# The container is debian:trixie to match the Pi's own base: the binary
# links against that glibc, so building anywhere newer would produce
# something the appliance refuses to start.
#
# Usage: scripts/pi-image/cross-build.sh [out-path]
# Env:
#   CARGO_CACHE  where the container's cargo/rustup homes live, so a second
#                run is incremental (default ~/.cache/tributary-cross)
set -euo pipefail

readonly TARGET="aarch64-unknown-linux-gnu"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly REPO
readonly OUT="${1:-$REPO/target/$TARGET/release/tribd}"
readonly CARGO_CACHE="${CARGO_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/tributary-cross}"

fail() { echo "cross-build.sh: $*" >&2; exit 1; }

command -v docker >/dev/null || fail "docker is required to cross-build"
docker info >/dev/null 2>&1 || fail "docker is installed but not usable by this user"

# The toolchain pin has one home; read it rather than restate it.
TOOLCHAIN="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$REPO/rust-toolchain.toml")"
[ -n "$TOOLCHAIN" ] || fail "no channel in rust-toolchain.toml"

# embed-ui bakes web/dist into the binary; a stale or missing dist would
# silently ship the wrong console, so build it here rather than hope.
echo "==> building web console"
if [ ! -d "$REPO/web/node_modules" ]; then
    ( cd "$REPO/web" && npm install )
fi
( cd "$REPO/web" && npm run build )

mkdir -p "$CARGO_CACHE/cargo" "$CARGO_CACHE/rustup"

echo "==> cross-compiling tribd for $TARGET (rust $TOOLCHAIN, debian trixie)"
docker run --rm \
    --platform linux/amd64 \
    -v "$REPO:/repo" \
    -v "$CARGO_CACHE/cargo:/cargo" \
    -v "$CARGO_CACHE/rustup:/rustup" \
    -e CARGO_HOME=/cargo \
    -e RUSTUP_HOME=/rustup \
    -e CARGO_TARGET_DIR=/repo/target \
    -e CARGO_BUILD_TARGET="$TARGET" \
    -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    -e PKG_CONFIG_ALLOW_CROSS=1 \
    -e PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig \
    -w /repo \
    debian:trixie-slim bash -euc "
        export DEBIAN_FRONTEND=noninteractive
        # Only /cargo and /rustup persist between runs — the container is
        # fresh every time, so its trust store and dpkg state are not
        # cached and both guards below would always miss. ca-certificates
        # in particular has to be unconditional: with a warm toolchain
        # cache the old code skipped installing it, and the first NEW
        # crate that needed downloading died on a TLS handshake. Nothing
        # catches that until a dependency is added, which is the worst
        # possible time to be debugging the build container.
        # ALSA headers are for the TARGET arch — cpal needs them, and the
        # host copy is the wrong architecture.
        dpkg --add-architecture arm64
        apt-get update -qq
        apt-get install -y -qq ca-certificates curl \
            build-essential gcc-aarch64-linux-gnu pkg-config libasound2-dev:arm64 >/dev/null
        if [ ! -x /cargo/bin/cargo ]; then
            curl -fsSL https://sh.rustup.rs \
                | sh -s -- -y --no-modify-path --default-toolchain '$TOOLCHAIN' >/dev/null
        fi
        export PATH=/cargo/bin:\$PATH
        rustup target add '$TARGET' >/dev/null
        cargo build --release -p tribd --features embed-ui
    "

[ -f "$OUT" ] || fail "expected binary at $OUT"
file "$OUT" | grep -q 'ELF 64-bit.*aarch64' || fail "$OUT is not an aarch64 ELF"
echo "==> $OUT"
file "$OUT" | sed 's/^/    /'
