# Third-party notices

Tributary's own code is MIT-licensed (see [LICENSE](LICENSE)). The following
third-party work is redistributed inside release artifacts:

## Fonts (embedded in the web console build)

- Barlow, Barlow Condensed — SIL Open Font License 1.1
- JetBrains Mono — SIL Open Font License 1.1
- Permanent Marker — Apache License 2.0

Bundled via the `@fontsource/*` packages, which carry the upstream license texts.

## DSP

- The reverb in `crates/trib-dsp/src/reverb.rs` uses the tunings of Jezar
  Wakefield's Freeverb, released to the public domain.

Rust crate and npm package dependencies are fetched at build time under their
respective licenses; see `Cargo.toml` and `web/package.json`.
