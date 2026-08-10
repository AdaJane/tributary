# Canonical dev ports, the single home for the pair: the daemon is told its
# bind, the UI is told the daemon's URL and pinned to its own port
# (--strictPort — 5180 stays clear of the 5173+ range other Vite apps hop
# through). Override ad hoc: `just daemon_port=4601 up`.
daemon_port := "4600"
ui_port := "5180"

# The bar for a PR: `just check` green.
check: fmt-check clippy test web-test

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

# Regenerate the committed OpenAPI spec (CI fails on drift).
openapi:
    cargo run -p tribd -- openapi > openapi.json

# Self-contained release build: the web console embedded in one tribd binary.
dist:
    cd web && npm run build
    cargo build --release -p tribd --features embed-ui
    @echo "==> target/release/tribd"

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
