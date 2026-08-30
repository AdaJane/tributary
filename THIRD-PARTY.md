# Third-party notices

Tributary's own code is MIT-licensed (see [LICENSE](LICENSE)). The following
third-party work is redistributed inside release artifacts:

## Fonts (embedded in the web console build)

- Barlow, Barlow Condensed — SIL Open Font License 1.1
- JetBrains Mono — SIL Open Font License 1.1
- Permanent Marker — Apache License 2.0

Bundled via the `@fontsource/*` packages, which carry the upstream license texts.

<!-- BEGIN soundfonts -->
## SoundFonts

Installed to `/usr/share/tributary/soundfonts` and listed in the
console as built-in sounds. Fetched at build time and checked
against `soundfonts/manifest.toml`.

- **GeneralUser GS** 2.0.3 by S. Christian Collins — GeneralUser GS License v2.0. <https://github.com/mrbumpy409/GeneralUser-GS/blob/main/documentation/LICENSE.txt>
- **FluidR3 GM** 3.1 by Frank Wen (2000-2002, 2008), Toby Smithe (2008) — MIT. <https://metadata.ftp-master.debian.org/changelogs/main/f/fluid-soundfont/fluid-soundfont_3.1-6_copyright>
- **MuseScore General** 0.2 by S. Christian Collins — MIT. <https://github.com/musescore/MuseScore/blob/master/share/sound/FluidR3Mono_License.md>
<!-- END soundfonts -->

## DSP

- The reverb in `crates/trib-dsp/src/reverb.rs` uses the tunings of Jezar
  Wakefield's Freeverb, released to the public domain.

Rust crate and npm package dependencies are fetched at build time under their
respective licenses; see `Cargo.toml` and `web/package.json`.
