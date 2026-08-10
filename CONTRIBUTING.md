# Contributing

Dev loop:

```sh
just up      # daemon on :4600, console on http://localhost:5180
just check   # the PR bar: rustfmt, clippy -D warnings, cargo test, web tests
```

If a change touches the API surface, regenerate the contracts and commit
them — CI fails on drift:

```sh
just openapi                 # openapi.json from the daemon
cd web && npm run codegen    # src/api/generated/schema.d.ts from the spec
```

Notes:

- `cargo build -p tribd --no-default-features` builds without ALSA headers
  (fake backend: test tone, no devices).
- The web console needs Node ≥ 20.19 (`npm install` inside `web/`).
- UI work should follow `design-system/tributary/MASTER.md`.
