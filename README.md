# Tributary

Multi-track live audio recording and mixing, shaped like the console you
already know: strips left to right, gain, EQ, sends, a fader, and a piece of
masking tape with the channel name on it.

A Rust daemon (`tribd`) owns all real-time audio — patching, mixing, the FX
loop, recording, and playback. The browser console is a view onto it over
REST + one WebSocket.

The Tracks view is a multitrack editor over the latest take: waveform lanes
(peaks computed by the take writer, streamed live while recording), a
frame-accurate playhead, click-to-seek, loop-region playback, and per-lane
solo/mute. Playback always renders in the daemon; its **monitor output** is
switchable between the console's hardware out and a browser stream —
48 kHz stereo f32 frames over a dedicated binary WebSocket (`/ws/monitor`)
played through an AudioWorklet. (Browsers can't receive raw UDP; WebRTC is
the documented upgrade path if sub-100 ms latency ever matters.)

## Install

All builds are Linux, x86_64 + aarch64, from the
[releases page](https://github.com/AdaJane/tributary/releases) unless noted.
Every asset carries GitHub build provenance — verify with
`gh attestation verify <file> --repo AdaJane/tributary`.

### Debian / Ubuntu (apt — gets upgrades)

```sh
curl -fsSL https://adajane.github.io/tributary/tributary.asc \
  | sudo gpg --dearmor -o /usr/share/keyrings/tributary.gpg
echo "deb [signed-by=/usr/share/keyrings/tributary.gpg] \
  https://adajane.github.io/tributary/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/tributary.list
sudo apt update && sudo apt install tributary
```

Installs `tribd` as a system service, enabled and started — console at
<http://127.0.0.1:4600>. One-off installs work too: grab the `.deb` from
the releases page and `sudo apt install ./tributary_*.deb`. The system
service sees no user Pulse session, so its patchbay lists raw ALSA
devices; run `tribd` from a terminal instead if you want the
friendly-named desktop sources.

### Fedora / openSUSE

`sudo dnf install ./tributary-*.rpm` from the releases page. The service
ships preset-disabled per RPM convention:
`sudo systemctl enable --now tribd`.

### Raspberry Pi (the appliance)

Flash `tributary-<version>-pi.img.xz` to an SD card — it's stock Raspberry
Pi OS Lite (arm64: Pi 3/4/5/Zero 2 W) with Tributary and PipeWire
preinstalled. For Raspberry Pi Imager's hostname/Wi-Fi/user dialog, launch
it as `rpi-imager --repo <URL of tributary-<version>-pi-imager.json>` (the
JSON is a release asset); plain "Use custom" flashing works too — for
headless Wi-Fi then, edit `user-data` on the boot partition. First boot
expands the card and starts the console **LAN-open** at
<http://tributary.local:4600> — no setup wizard, no prompts. A plain flash
ships **no login account**: nobody can log in on the console or over SSH
until you reflash with Imager customization (which creates your user and
can enable SSH). Recordings live under `/home/tributary/projects`; logs
via `sudo journalctl SYSLOG_IDENTIFIER=tribd` (needs a login user). Read
Security below — the appliance trusts its LAN.

### Docker

```sh
docker run --rm -p 4600:4600 -v tributary-projects:/projects \
  --device /dev/snd --group-add audio ghcr.io/adajane/tributary:latest
```

That's raw ALSA. For the friendly-named Pulse/PipeWire sources, mount your
session socket instead of the device:
`-e PULSE_SERVER=unix:/pulse -v $XDG_RUNTIME_DIR/pulse/native:/pulse`.

### AppImage / tarball

Single-file builds; the only host requirement is ALSA (`libasound2`):

```sh
chmod +x tributary-*.AppImage && ./tributary-*.AppImage
# or
tar xzf tributary-*.tar.gz && ./tributary-*/tribd
```

The daemon serves the console at <http://127.0.0.1:4600>. Recordings land
in `projects/` next to where you launch it; change the destination in the
Setup tab. For the friendly-named patchbay sources install
`pulseaudio-utils` (`pactl`/`parec`) — present by default on PipeWire
desktops; without a Pulse server the daemon falls back to raw ALSA/cpal
devices.

## Security

The daemon has **no authentication by design**: it trusts the machine — and
when bound beyond loopback, the network — it is reachable on. Browser
origins are fenced in three tiers: loopback passes on any port; a
same-origin page whose host is an mDNS `.local` name or an IP literal
passes (that's the Pi appliance and LAN case — public DNS can't serve
those hosts, which keeps DNS-rebinding pages out); anything else must be
listed in `server.cors_origins` (reverse proxies, custom DNS names). The
default bind is `127.0.0.1`. Don't bind a non-loopback address
(`TRIB__SERVER__BIND=0.0.0.0:…`) on a network you don't fully trust — that
exposes an unauthenticated control surface that can capture audio and
write to disk. The Raspberry Pi appliance image ships LAN-open on this
doctrine deliberately: a studio LAN is its trust boundary.

## Developing

```sh
just up         # daemon (:4600) + Vite dev console (http://localhost:5180)
just run        # just the daemon (config in config/tribd.toml)
just web-dev    # just the console UI
just check      # fmt + clippy + tests + web tests — the PR bar
just dist       # release build: console embedded in a single tribd binary
```

Building needs Rust (the pinned toolchain in `rust-toolchain.toml` is picked
up automatically), Node ≥ 20.19, and the ALSA headers:
`sudo apt install libasound2-dev`.

## Inputs

Strips patch from **any input source on the system** — the patchbay lists
the same friendly-named sources as the OS Sound panel, one lettered stage
box each (A is always the system default input, annotated with the source
it currently follows). Sources are enumerated from the Pulse/PipeWire
server (`pactl list sources`) and captured per source by a `parec` child
process at the engine rate — the server resamples, so mismatched hardware
rates just work; without a pulse server the daemon falls back to direct
ALSA/cpal devices. Streams open on demand when first patched and close
when the last strip unpatches; the modal's Refresh re-enumerates and
retries anything that failed (plug in, hit Refresh). Patches identify
sources by OS name; on restart, a renamed device (`… #2`-style replug
drift) is adopted automatically when the match is unambiguous — otherwise
the patch shows `not connected` rather than guessing at the wrong
microphone. The output device remains the system default. The engine is
clocked by its own timer thread, never by the output device: a stalling
or vanishing output (Bluetooth renegotiation, route changes) costs
monitor audio only — metering and recording continue — and the monitor
stream rebuilds itself on the current default once one is healthy.

## Layout

- `crates/trib-core` — pure mixer domain (state, signal graph, reducer)
- `crates/trib-dsp` — DSP building blocks (filters, dynamics, reverb)
- `crates/trib-engine` — realtime mix engine (strips, buses, tape return)
- `crates/trib-audio` — device IO (cpal/ALSA backend, Pulse capture)
- `crates/trib-project` — persistence (takes, manifests, peaks sidecars)
- `crates/tribd` — the daemon: axum API, WS hub, engine host
- `web/` — React console UI
- `design-system/tributary/MASTER.md` — the design system spec
- `openapi.json` — generated by `tribd openapi`, consumed by the web codegen

## License

MIT — see [LICENSE](LICENSE). Attributions for bundled fonts and DSP
tunings are in [THIRD-PARTY.md](THIRD-PARTY.md).
