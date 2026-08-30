# Canonical dev ports, the single home for the pair: the daemon is told its
# bind, the UI is told the daemon's URL and pinned to its own port
# (--strictPort — 5180 stays clear of the 5173+ range other Vite apps hop
# through). Override ad hoc: `just daemon_port=4601 up`.
daemon_port := "4600"
ui_port := "5180"

# The bar for a PR: `just check` green.
check: fmt-check clippy test web-test script-test attribution-check

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

test:
    cargo test --workspace

web-test:
    cd web && npm run -s test

# The image helpers' pure derivations, plus shellcheck over every script.
# These run at boot or at hotplug on a box with no console, so a rotted
# derivation surfaces as silence on hardware — not as a failing test.
script-test:
    #!/usr/bin/env bash
    # `A && B || C` would swallow shellcheck's verdict into the skip branch
    # — the very SC2015 pattern it warns about. Gate at warning: the info
    # level flags style notes in scripts that already work.
    set -euo pipefail
    scripts/pi-image/test-helpers.sh
    if command -v shellcheck >/dev/null; then
        shellcheck --severity=warning scripts/pi-image/*.sh \
            packaging/deb/postinst packaging/deb/postrm
    else
        echo "shellcheck not installed — skipped"
    fi

# Download the bundled SoundFonts into soundfonts/dist and verify every
# byte against soundfonts/manifest.toml. Idempotent; ~380 MB on a cold run.
# Needed before `just pi-image` or any packaging that ships them.
soundfonts:
    scripts/fetch-soundfonts.sh

# Rewrite THIRD-PARTY.md's SoundFonts section from the manifest.
attribution:
    scripts/update-attribution.sh

# A bundled sound's licence has one home. This is a drift gate like the
# openapi one: a font added to the manifest but never attributed would
# otherwise ship with nothing to notice.
attribution-check:
    scripts/update-attribution.sh --check

# Regenerate the committed OpenAPI spec (CI fails on drift).
openapi:
    cargo run -p tribd -- openapi > openapi.json

# Self-contained release build: the web console embedded in one tribd binary.
dist:
    cd web && npm run build
    cargo build --release -p tribd --features embed-ui
    @echo "==> target/release/tribd"

# Needs sudo, ~4 GB free, and an aarch64 tribd; on an x86_64 box install
# qemu-user-static (binfmt) and feed it a binary from a previous release.
# Pass --no-compress to skip the slow xz while iterating.
# AP_COUNTRY picks the Wi-Fi regulatory domain (`AP_COUNTRY=GB just
# pi-image`); it is forwarded explicitly because sudo resets the
# environment, and its default lives in build.sh.
# Remaster the Pi appliance image locally (rehearsal — no tag needed).
pi-image binary="target/release/tribd" *flags="":
    sudo --preserve-env=AP_COUNTRY,CACHE_DIR,IMAGE_URL_BASE \
        scripts/pi-image/build.sh {{binary}} target/tributary-dev-pi.img.xz {{flags}}

# The Pi the dev recipes talk to. Override: `just pi=192.168.1.50 pi-deploy`.
pi := "tributary.local"
pi_user := "dev"
ssh_key := env_var('HOME') / ".ssh/id_ed25519.pub"

# Built to target/aarch64-unknown-linux-gnu/release/tribd. Runs in a
# container because this host has no aarch64 target or linker.
# Cross-build an aarch64 tribd (web console embedded) for the Pi.
pi-tribd:
    scripts/pi-image/cross-build.sh

# The appliance plus an SSH login (your key, key-only), audio tooling and
# the trib-dev helper. Ethernet is the SSH path — the Wi-Fi AP still owns
# wlan0, exactly as in production. Emitted uncompressed: flashing a raw
# .img beats waiting for xz while iterating.
# Build a DEBUGGING Pi image with SSH. Never tag a release from it.
pi-image-dev: pi-tribd
    sudo --preserve-env=AP_COUNTRY,CACHE_DIR,DEV_SSH_KEY,DEV_USER \
        env DEV_SSH_KEY={{ssh_key}} DEV_USER={{pi_user}} \
        scripts/pi-image/build.sh \
            target/aarch64-unknown-linux-gnu/release/tribd \
            target/tributary-ssh-dev-pi.img.xz --no-compress
    @echo "==> flash target/tributary-ssh-dev-pi.img, then: ssh {{pi_user}}@{{pi}}"

# No reflash — seconds instead of a quarter of an hour. Needs a dev image.
# Rebuild tribd and push it to a running dev Pi.
pi-deploy: pi-tribd
    scp target/aarch64-unknown-linux-gnu/release/tribd {{pi_user}}@{{pi}}:/tmp/tribd
    ssh {{pi_user}}@{{pi}} 'sudo install -m 755 /tmp/tribd /usr/local/bin/tribd \
        && rm -f /tmp/tribd && sudo trib-dev restart && sleep 1 && sudo trib-dev status --no-pager'

# The first thing to look at when channels are wrong or missing.
# Print the daemon's own input report, straight off the Pi.
pi-devices:
    @ssh {{pi_user}}@{{pi}} 'curl -fsS http://127.0.0.1/api/v1/devices' | python3 -m json.tool

# Follow tribd's log on the Pi.
pi-logs:
    ssh {{pi_user}}@{{pi}} 'sudo trib-dev logs -f'

# Local package builds — the same tools CI runs (which passes --no-build
# and packages the release leg's own embed-ui binary).
deb: dist
    cargo deb -p tribd --no-build
    @echo "==> target/debian/"

rpm: dist
    cargo generate-rpm -p crates/tribd
    @echo "==> target/generate-rpm/"

# Bring up the whole stack: daemon + web UI, together. Ctrl-C stops both.
up:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -d web/node_modules ]; then
        echo "==> installing web dependencies (first run)…"
        (cd web && npm install)
    fi
    echo "==> daemon on http://127.0.0.1:{{daemon_port}} · UI on http://localhost:{{ui_port}} (Ctrl-C stops both)"
    trap 'kill 0' EXIT INT TERM
    TRIB__SERVER__BIND=127.0.0.1:{{daemon_port}} cargo run -p tribd &
    (cd web && VITE_TRIBD_URL=http://127.0.0.1:{{daemon_port}} npm run dev -- --port {{ui_port}} --strictPort) &
    wait -n

run:
    TRIB__SERVER__BIND=127.0.0.1:{{daemon_port}} cargo run -p tribd

web-dev:
    cd web && VITE_TRIBD_URL=http://127.0.0.1:{{daemon_port}} npm run dev -- --port {{ui_port}} --strictPort
