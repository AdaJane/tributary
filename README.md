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
Pi OS Lite (arm64: Pi 4 / 400 / CM4 / Pi 5) with Tributary and PipeWire
preinstalled. First boot expands the card and needs no setup wizard and no
prompts:

1. The appliance **hosts its own Wi-Fi network**, named `Tributary-XXXX`
   (the suffix is your Pi's serial, so two units in one room stay apart).
   The default passphrase is `tributary`.
2. Join it and open **<http://tributary.local>** — no port to type.

Plug in ethernet and the console is reachable that way too, at the same
address; AP clients reach the wired network through the appliance.

**The access point owns the Wi-Fi radio**, so Raspberry Pi Imager's Wi-Fi
credentials no longer do anything — a Pi has one radio and it cannot host
this network and join another at the same time. To put the appliance on an
existing network, use ethernet. Imager's hostname/user dialog still works
(launch it as `rpi-imager --repo <URL of tributary-<version>-pi-imager.json>`,
a release asset); plain "Use custom" flashing works too.

To change the Wi-Fi passphrase, SSID prefix, channel or address, edit
`scripts/pi-image/tributary-ap.nmconnection` and rebuild the image
(`just pi-image`); the regulatory domain defaults to `US` and is a build
input — `AP_COUNTRY=GB just pi-image` for a regional image.

A plain flash ships **no login account**: nobody can log in on the console
or over SSH until you reflash with Imager customization (which creates your
user and can enable SSH). Recordings live under `/home/tributary/projects`;
logs via `sudo journalctl SYSLOG_IDENTIFIER=tribd` (needs a login user).
Read Security below — the appliance trusts its own network.

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

On the appliance that trust boundary is **its own Wi-Fi network**, and the
WPA2 passphrase is the only thing in front of an unauthenticated console —
anyone who joins can start and stop recordings and listen to the monitor
stream. The image's default passphrase (`tributary`) is published here, so
treat it as public: change it and rebuild before using the appliance
anywhere you wouldn't hand out the key. Note also that the AP shares a
plugged-in ethernet connection with its clients, so joining the appliance's
network means reaching the wired one behind it.

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

### Debugging on real Pi hardware

The appliance image ships no login account, which makes it awkward to debug
audio problems that only appear on the device. There is a **dev image** for
that — the same appliance plus an SSH login, audio tooling and a helper:

```sh
just pi-image-dev          # dev image, your ~/.ssh/id_ed25519.pub baked in
```

Flash `target/tributary-ssh-dev-pi.img`, connect **ethernet** (the Wi-Fi AP
still owns `wlan0`, exactly as in production), then:

```sh
just pi-deploy             # rebuild tribd and push it — no reflash, seconds
just pi-devices            # the daemon's own input report, as JSON
just pi-logs               # follow tribd's journal
```

All three take `pi=<host-or-ip>` (default `tributary.local`).

On the Pi, `trib-dev` runs things **inside the service user's session**,
which is the only place audio questions get truthful answers — `pactl` typed
at a plain SSH prompt talks to a different PipeWire than the one tribd
captures from:

```sh
trib-dev sources           # the capture sources tribd can actually see
trib-dev cards             # cards and profiles — the channel-count setting
trib-dev run pw-top        # or any other command in that session
```

Cross-compilation runs in a container (`scripts/pi-image/cross-build.sh`),
so no aarch64 toolchain is needed on the workstation — only Docker. Never
tag a release from a dev image: it carries a login account, and the release
build asserts that it doesn't.

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
microphone. The engine is clocked by its own timer thread, never by the
output device: a stalling or vanishing output (Bluetooth renegotiation,
route changes) costs monitor audio only — metering and recording continue
— and the monitor stream rebuilds itself on the current default once one
is healthy.

## Outputs

Any channel on the desk — a strip, a bus, or the final mix — can be wired
straight to a physical output as **pass-through**, without going through
the master first. The **OUT** button on a channel strip (and on the master)
opens the output patch bay.

The room is organised by OUTPUT, not by source, because that is the way the
cardinality runs: one output channel carries one signal, while one source
can feed several. Patching an output that is already in use replaces what
was there, and the jack prints its current holder before you click. It is
also what makes the master and the buses reachable at all — they appear in
the source list rather than needing channel strips they do not have.

Each patch taps its source **pre-fader** by default: post-gain, post-EQ,
and before the mute and fader — the same point the tape and the meters
take. A front-of-house fader move then cannot change what a monitor
engineer is hearing. The per-patch **Pre/Post** switch moves it, and unlike
patching itself it stays live while the tape is rolling.

Two things a direct out is deliberately immune to: pressing **PFL** does
not reach it (soloing a kick to check it must not send the kick to the PA),
and neither does playing back a take. Both of those replace the *monitor*
feed, which is a different thing from a feed to the room.

Patching is refused while recording, with the reason shown. Re-pointing a
jack costs a graph recompile, and a recompile clears FX tails into the
take — so the refusal protects the recording, not the implementation.

## Instruments

Not every source is outside the box. The **Instruments** tab holds a rack of
SoundFont players: pick a `.sf2`, pick a preset, point it at a MIDI keyboard,
and it becomes an input source like any other — patched from the same
patchbay, with the same gain, EQ, fader and record arm as a microphone. From
the strip downward nothing knows the difference, which is why instrument
channels record, meter, play back and draw waveform lanes with no special
handling anywhere.

**Outputs.** An instrument reaches the desk either as one **stereo pair** —
what a piano wants — or as **per-drum splits**: named slices of the keyboard
(Kick, Snare, Toms, HiHat, Cymbals, Percussion), each its own mono channel
with its own fader and pan. A SoundFont kit is one MIDI channel with a
different drum on every key, so the key number is the only thing that can
separate them; the General MIDI map interleaves toms and hi-hats, which is
why a split holds a list of key ranges rather than one. **Add all channels
to mixer** then creates and names a strip for every output in one move —
six drums, six faders, ready to balance before anyone plays. It adds only
what is missing, so pressing it twice costs nothing and levels you have
already set survive.

Splits share one loaded soundfont and only ever sound their own keys, so a
six-piece kit costs no extra sample memory and no extra voices — just one
synthesiser instance each.

**Soundfonts** live in `soundfonts.root` on the boot disk (the appliance
uses `/var/lib/tributary/soundfonts`), and any mounted drive is scanned one
level deep, so dropping files on a USB stick is enough. The console can also
upload one directly. Note the memory cost: all sample data stays resident, so
`soundfonts.max_bytes` (64 MB by default) is a RAM budget, not a disk one — a
148 MB General MIDI set is an OOM kill on a 2 GB Pi 4 rather than a slow
load. None is bundled; Tributary ships no soundfont of its own.

**MIDI** comes in over the ALSA sequencer, so any USB keyboard the system
enumerates works, and no extra package is needed. Ports open when an
instrument names one and close when none does; nothing retries in the
background, so plugging a keyboard in and pressing Refresh is the whole
recovery story. An instrument that names a port which is not there says so
rather than guessing at a different one.

**Takes carry the notes too.** Recording an armed instrument channel writes
its audio exactly like any other track, and drops a Standard MIDI File beside
it (`inst01-kit-kick.mid`), listed in `take.toml` under `[[midi_tracks]]`. The
audio stays authoritative — a sidecar that fails to write never marks a take
damaged, because "the audio is not what the room heard" and "you have the
audio but not the notes" are different problems. Lanes carrying one show a
`MIDI` chip in the Tracks view.

**Latency, honestly.** On the shared audio layer — any desktop install —
playing an instrument live goes through the engine block and the monitor's
output prefill: roughly 50–70 ms end to end. That is fine for pads and
workable for parts, and too slow for fast keyboard work. The prefill is
what keeps a stalling output device from freezing metering and recording,
so it is not lowered by default.

The appliance runs the **exclusive** layer instead: one ALSA card opened
duplex and clock-linked, the engine clocked by the card, and no server in
the path. That removes the prefill, the resampling and the drift between
capture and playback — but the number it lands on depends on the interface
and has not been measured on hardware yet, so this README does not quote
one. `GET /api/v1/outputs` reports the worst engine block per second and
the xrun count; those are the numbers to judge it by.

## Layout

- `crates/trib-core` — pure mixer domain (state, signal graph, reducer)
- `crates/trib-dsp` — DSP building blocks (filters, dynamics, reverb)
- `crates/trib-engine` — realtime mix engine (strips, buses, tape return,
  the SoundFont instrument rack)
- `crates/trib-audio` — device IO (cpal/ALSA backend, Pulse capture)
- `crates/trib-project` — persistence (takes, manifests, peaks and MIDI sidecars)
- `crates/tribd` — the daemon: axum API, WS hub, engine host
- `web/` — React console UI
- `design-system/tributary/MASTER.md` — the design system spec
- `openapi.json` — generated by `tribd openapi`, consumed by the web codegen

## License

MIT — see [LICENSE](LICENSE). Attributions for bundled fonts and DSP
tunings are in [THIRD-PARTY.md](THIRD-PARTY.md).
