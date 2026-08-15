# Tributary Design System — MASTER

The canonical spec every UI change consults. The machine source of truth for
values is `web/src/design/tokens.css`; this document carries the rationale,
component contracts, and interaction rules. If the two disagree, fix the drift
in the same PR.

## Philosophy

**Retro early-2000s console hardware, restrained skeuomorphism.** The UI reads
as a Soundcraft/Mackie-era desk: charcoal powder-coat panels, screen-printed
white labels, colored plastic caps, real LEDs, masking tape + sharpie. But it
is drawn entirely in vectors and CSS:

- **No bitmap textures. No blur filters.** Depth = 1px etch lines
  (`--etch-hi`/`--etch-lo`) plus at most one inner shadow, via the four depth
  recipes (`--depth-raised`, `--depth-inset`, `--depth-cap`, `--depth-pop`).
- **Dark-only** (`color-scheme: dark`). A console lives in a dim room.
- **Dense** (8px scale with 2px sub-step). Real strips are 40mm wide; screen
  strips are `--strip-w`.
- **Motion is subtle** (90–240ms, `--ease-out`) and **LEDs/meters are exempt:
  they snap** like hardware. No transition on any LED state change.
- **Hardware honesty**: controls look like what they do. A latched button
  depresses; a knob's pointer rotates; tape is slightly crooked
  (deterministic per-channel rotation, ±1.2°, so it never re-shuffles).

## Token layers

Three layers in `tokens.css`; lower layers never reference higher ones.
Components reference **semantic and component tokens only** — never a `--gray-*`
or `--cap-*` primitive directly, and never a raw color. Enforced by
`web/src/design/token-guard.test.ts` (fails the suite on raw hex,
`rgb()`/`hsl()`/`oklch()`, or literal font families outside `tokens.css`).

### Primitives (raw material)

| Group | Tokens | Notes |
|---|---|---|
| Panel grays | `--gray-0` … `--gray-9`, `--print-white` | `--gray-7` is the dimmest text allowed on `--surface-strip` (4.6:1) |
| Cap plastics | `--cap-{red,blue,yellow,green,white,grey}` (+`-hi`) | Soundcraft palette; `-hi` = hover/drag brighten |
| LEDs | `--led-{green,amber,red}-{on,off}` | "off" is dim colored plastic, never invisible |
| Tape | `--tape-cream`, `--tape-cream-edge`, `--ink` | ink on tape ≈ 11:1 |
| Type | `--font-ui` Barlow · `--font-label` Barlow Condensed · `--font-tape` Permanent Marker · `--font-mono` JetBrains Mono | sizes `--text-2xs`(9) … `--text-lg`(15); print labels are uppercase + `--track-print` |
| Space / radii / motion | `--s0`(2) … `--s6`(32) · `--radius-1..3,round` · `--dur-1..3`, `--ease-out` | |

### Semantic (roles)

Surfaces `--surface-{app,console,strip,section,control,inset}`; borders
`--border-{panel,divider}`; text `--text-{label,value,dim}`; states
`--state-{record,solo,mute,active}`; focus `--focus-color`/`--focus-ring`
(amber, two-layer so it reads on any surface); depth recipes and LED glows
(`--glow-{green,amber,red}`).

### Component

Knob (`--knob-*`, 270° sweep, detent up), fader (`--fader-*`, 200px throw),
LED (`--led-size` 8px), buttons (`--btn-*`), select (`--select-h` 32px),
tape (`--tape-*`), strip/master
widths (`--strip-w`, `--master-w`). `@media (pointer: coarse)` bumps sizes to
44px-class targets; desktop dense mode guarantees ≥24px visual + ≥32px hit
area (WCAG 2.2 minimum) via padding overlays.

## Component contracts

All generic controls live in `web/src/design/`, know nothing about the daemon,
and take plain props. States below are **default / hover / drag / focus /
disabled** unless noted.

### Knob
- Anatomy: body circle (`--knob-body`, `--depth-raised`), colored cap dot
  (`capColor` prop, `--depth-cap`), white pointer line, printed tick arc +
  min/center/max legends (`--text-2xs`, `--text-dim`).
- Value model: `{ min, max, value, defaultValue, taper, format }`.
- Interaction: pointer-capture **vertical drag** (150px = full sweep);
  **Shift = ×0.1 fine**; **double-click = reset to default**; keys: arrows =
  1 step, PageUp/Down = 10, Home/End = min/max. No pointer lock. During drag:
  `ns-resize` cursor, mono value readout above, no transitions on the pointer.
- ARIA: `role="slider"`, `aria-orientation="vertical"`,
  `aria-valuemin/max/now`, `aria-valuetext` from `format` ("+2.5 dB",
  "1.2 kHz"), explicit `aria-label` ("Channel 3 gain").
- States: hover brightens cap to `-hi`; focus = `--focus-ring`; disabled =
  45% opacity, no pointer events, `aria-disabled`.

### Fader
- Inset slot (`--surface-inset`, `--depth-inset`) + printed dB scale (+10 to
  −∞; 0 marked bold) + cap (`--fader-cap-face`, grip line in strip cap color).
- Audio taper, pure module `audio/taper.ts`: position↔dB piecewise
  `(1,+10) (0.75,0) (0.5,−10) (0.25,−30) (0.05,−60) (0,−∞)` — unity at 75%
  travel. At/below `FADER_MIN_DB` (−90) render "−∞".
- Interaction/ARIA: same contract as Knob, plus click-on-track = jump,
  drag cursor `grabbing`, double-click cap = unity (0 dB).

### LedMeter
- Channel variant: 7 LEDs bottom-up at −40 −30 −20 −12 (green) · −6 −3
  (amber) · 0 (red). Master variant: 12 per side, green→−12, amber→−3, red
  above. Thresholds live in `audio/leds.ts` — this spec is their source of
  truth; the daemon owns only `CLIP_DB`.
- LED = rounded square `--led-size`; off = `--led-*-off` + `--depth-inset`;
  on = `--led-*-on` + `--glow-*`. **Never transitioned.**
- Clip: server detects (`clip` on the meter frame — it saw every sample);
  client latches `CLIP_HOLD_MS = 1800`, click clears early. When clipped, the
  meter is a button ("clear clip"), otherwise not a tab stop.
- ARIA: `role="meter"`, `aria-valuemin/max/now` in dB, label "Channel N level".
- Color is never the only signal: master clip also prints "CLIP".

### PushButton (latching)
- Cap `--btn-h` high, `--depth-raised`, print label (`--font-label`,
  uppercase, `--track-print`), integrated LED dot (`--btn-led-size`).
- `led={false}` hides the dot when the status lamp lives elsewhere (the EQ
  On switch defers to the section-header dot); the cap still depresses, so
  state is never color-only.

### ActionButton (momentary)
- Same cap recipe as PushButton but springs back: no latch, no
  `aria-pressed`, no LED. For fire-and-forget actions (EQ Reset).
- Variants: `mute` (red LED), `pfl` (yellow), `arm` (red; **blinks 1 Hz while
  transport records**, steady when armed idle; under reduced-motion the blink
  falls back to steady + label swaps to "REC").
- Latched: cap translates down 1px, `--depth-inset`, LED on + glow.
- `<button aria-pressed>`; hit area ≥32×32 via `::before` overlay.

### TapeLabel
- `--tape-cream` strip, `--tape-cream-edge` torn ends via `clip-path` nicks,
  rotation = deterministic hash of the id within ±`--tape-rotate-max`.
- The tape is a fixed two-line box (`--tape-h`). Names WRAP, and a hidden
  measuring twin (use-fit-text) shrinks the writing (100%→50%) until the
  whole name fits — no ellipsis, every word stays on the tape. The probe is
  keyed by the text so edits re-measure without remounting the field.
- Single click selects; double-click or Enter/F2 edits inline (a
  `<textarea>` styled identically, same wrap + shrink live while typing;
  newlines are refused); Enter commits (optimistic, rollback on error),
  Esc reverts; focus returns to the label.
- ARIA: a button labeled "Rename channel N, currently 'Kick'".

### CollapsibleSection (EQ)
- Header: print title + engaged-LED dot + chevron; `<button aria-expanded
  aria-controls>`. Collapsed shows a one-line mono summary
  ("HF +2 · 1.2k +4 · LF 0").
- Expanded EQ leads with a switch row: **On** (led-less PushButton — the
  header dot is its lamp) and **Reset** (ActionButton, all bands to flat,
  engagement untouched).
- Expansion animates `grid-template-rows` 0fr→1fr at `--dur-3`; instant under
  reduced motion. Expansion state is shared UI state (persisted per strip in
  `state/ui.ts`), default collapsed.

### Modal (design kit)
- Native `<dialog>` via `showModal()`: platform focus trap, Esc-to-close,
  focus return to the opener. Surface `--surface-section` + `--depth-pop`,
  backdrop `--backdrop`. Header = print title + Lucide X close.

### SegmentedControl (interlocked selector)
- A row of piano-key latching switches: exactly one cap stays down. The
  caps share the PushButton recipe (raised → pressed 1px + `--depth-inset`
  when checked, green LED + glow on the latched cap, LED snaps).
- Adjacent caps share a `--border-divider` seam and end-only radii so the
  row reads as one machined assembly.
- `role="radiogroup"` + per-option `role="radio" aria-checked`; roving
  tabindex (only the checked cap is tabbable), Arrow keys move AND select
  with wrap, Home/End jump. Clicking the checked cap is a no-op.
- Hit area ≥32px via a vertical-only `::before` overlay (no horizontal
  overhang — caps must not poach a neighbor's clicks).

### Panel / InlineError / Waiting (rear-panel kit)

- `Panel` is the card every rear-panel room is built from: `--surface-strip`,
  `--depth-raised`, a print header row (`--font-label`, uppercase,
  `--track-print`) with an optional right-aligned `badge` slot for a lamp or
  a count. Lifted out of SetupView when Instruments needed the same recipe —
  a panel card is a design fact, and a second copy of it in another
  stylesheet is drift waiting to happen.
- `InlineError`: red LED dot + sentence, `role="status"`, printed **inside
  the panel that failed**. Never a toast, never at the top of the page.
- `Waiting`: the body before the daemon has answered — "waiting for the
  daemon…", deliberately not a spinner, because nothing is spinning.

### SelectField (unbounded picker)

- A native `<select>` on `--surface-inset` + `--depth-inset`, `--select-h`
  tall. For lists the machine supplies and the user cannot be expected to
  scan: soundfont presets (hundreds), MIDI ports, MIDI channels.
- `SegmentedControl` stops working past about five options; this is what
  replaces it, and the two are not interchangeable — a fixed short set of
  choices is still piano keys.
- A value that is not in the list renders **blank and lies** — a `<select>`
  whose value matches no option displays the FIRST one — so the option list
  always appends the current value with `(not connected)` when the daemon
  no longer offers it. That rule lives in `select-options.ts`, not in each
  call site: it was reimplemented at one and omitted at another before the
  component existed.
- Implemented as `design/SelectField.tsx`. The two hand-rolled copies it
  replaced **disagreed** — one was a raised control where this spec asks
  for a recessed well.

### TextField (inset field)
- An inset machine-text well: `--surface-inset` + `--depth-inset`,
  `--font-mono`, `--btn-h` height — for machine text (paths), never names
  (names go on tape).
- TapeLabel's commit contract: Enter/blur commits the trimmed draft iff it
  differs, Esc reverts; a null draft means "not editing" so external value
  changes flow through until typing starts.

### InputPickerModal — the patchbay
- Trigger = strip-top INPUT button showing the current patch ("IN 3" on
  the default box, "B2" on stage box B, "—" unpatched),
  `aria-haspopup="dialog"`.
- One SECTION per input source — the same list as the OS Sound panel:
  sources come from the pulse server with friendly labels ("C922 Pro
  Stream Webcam Analog Stereo"), titled `label ?? name`. Stage-box
  lettering: A is always the system default input (routing aliases fold
  into it; patches carry `device: null`; header prints "follows:
  <active source label>"), then EVERY named source lettered B, C… in
  stable lexicographic order of `name` (`deviceLetters`) — including the
  one the default currently follows (A tracks the system; the named box
  pins it). Section header is a flex row, `--text-md` print: letter chip ·
  source-kind glyph (`SourceIcon`, drawn SVG like the socket — loop arrow
  for the default, mic / webcam / USB trident / quarter-inch plug guessed
  from the title by `sourceKind`, supplementary only) · title (A appends
  "↳ <followed source>" dim) · right-aligned LED dot + one-word status
  (`live` green / `ready` dim / `failed` red / `missing` amber) · "was:
  <old name>" when restart reconciliation adopted the device under a
  changed name. No jack count in print — the tiles are the count.
- Opening the modal calls `POST /api/v1/devices/refresh` — the daemon
  re-enumerates AND retries failed/absent wanted devices, so plugging in
  and reopening is enough. A Refresh ActionButton does the same on demand.
  Devices open on demand when first patched and close when the last strip
  unpatches.
- Jacks render as tiles (drawn XLR socket + print label), including jacks
  feeding other strips: holders' names in tape script plus the link-color
  dot. Free jacks say "free"; the strip's own patch renders selected
  (amber ring, lit pin). A patch past a device's channel count stays
  VISIBLE: dimmed socket, "no jack on this device" — a silent strip must
  be explainable from the patchbay. With enumeration unavailable, the
  default box draws ZERO jacks — inventing a count drew phantom sockets
  that looked exactly like a real device and hid the fact that enumeration
  had found nothing.
- An in-use jack is still selectable — sharing is legal, and is exactly
  what the tape stripe marks. Footer: hint text + Refresh + Disconnect
  (disabled when unpatched).

- **Instruments are sections too, and are never lettered.** They render
  after the hardware boxes with the same geometry, but where a letter chip
  would go they print a slot — `INST 3`. Lettering them would collide in
  print (box `I` against slot `I1`) and, worse, adding an instrument would
  re-letter the stage boxes, changing the colour and print of the link tape
  on two strips under the user's hands. `deviceLetters` therefore never
  sees an instrument. The glyph is a drawn piano keyboard (`SourceIcon`),
  and the tiles are the instrument's channels, named as the strips would be
  ("Kit Kick", "Rhodes L").
- **A removed instrument leaves a ghost section.** If a strip is still
  patched to an instrument the rack no longer holds, `groupPatchbay`
  synthesizes a section for it — status `missing`, "this instrument was
  removed — patch this channel somewhere else". Without it the strip's
  INPUT button prints `INST 3.1` with nothing anywhere to explain the
  silence, which is exactly the failure this modal exists to prevent.
- Instrument silence uses the same amber lead as a silenced device:
  `Silent at the source: no soundfont chosen — fix it in Instruments`. One
  ordered table of reasons, first true wins, rendered bare in the rack and
  suffixed with the door in the patchbay.

### Link tape (shared inputs)
- Strips fed by the same jack wear a matching strip of colored tape across
  their top edge (torn clip-path ends, `--depth-raised`, `--font-tape`
  label "IN n" / "B2" in `--ink`).
- A jack is `(device, channel)` — channel 0 on two devices is two
  different jacks with two different tapes. Color is keyed by the hashed
  jack identity (`stripeColor(jackKey(...))`), so a link keeps its color
  as strips join or leave.
- Color is never the sole signal: the tape prints its jack label, and each
  stripe carries an aria-label naming the share.
- **Outputs earn no tape.** Tape marks a shared identity between two
  strips; an output patch is always 1:1, so there is nothing invisible for
  it to reveal. Tape is also the console's scarcest signal — six colors,
  one edge per strip — and spending it on a relationship that is never
  shared devalues it for the one that is.

### OutputPatchbayModal — the output bay

The mirror of the input patchbay, and deliberately not its twin: **the
cardinality is inverted.** On the way in a STRIP is the "one" and jacks are
the "many", so the room opens from the one and lists the many. On the way
out an OUTPUT CHANNEL is the "one" — it holds at most one feed — so the
room lists outputs and picks a source.

That single decision is what makes the master and the buses reachable at
all: they appear in the **source list**, not as objects needing channel
strips they do not have. There are no bus strips in this console, and the
master's strip is the fixed right-hand panel, so a strip-only door could
never have reached either.

- **Two doors, no third.** An `OutputButton` on every ChannelStrip
  (mirroring INPUT, printing `—` / `OUT 3` / `2 OUTS`) and one in the
  MasterSection. Buses need no door of their own — they are in the source
  list. Opened from a door, clicking a FREE output patches it on the spot:
  two informed clicks. An occupied output only selects, so its holder is
  read before it is replaced.
- **A modal, not a full-page room.** A full-page tab renders INSTEAD of the
  pager and unmounts both live rooms, meters included — and you patch
  outputs while listening to what they carry.
- **Jack anatomy**: drawn socket → print label → status, exactly as on the
  input side, but with **three pins in a triangle** (male XLR) against the
  input jack's single centre pin. One CSS fact different, so the two rooms
  are never confusable at a glance.
- Where the input board prints a **letter**, an output section prints an
  `OUT` chip. **Outputs are never lettered**: a second alphabet would
  collide in print with the stage boxes' `B2`, and adding an output device
  must never re-letter an input jack under the user's hands — the same
  argument that keeps instruments unlettered.
- **Print numbering follows the hardware.** A device that gives channels to
  the monitor (the real-time backend shares one card) prints `OUT 3` for
  its first patchable jack, because that is the socket on the back panel.
- **Tap** is a two-cap `SegmentedControl` (Pre / Post), default Pre, with a
  live consequence line underneath. Pre is post-gain, post-EQ, **pre-mute
  and pre-fader**; Post is post-mute and post-fader but **pre-pan** — a
  strip is mono and so is its jack.
- **Honest failure, in full.** A ghost section for an unplugged output a
  patch still names ("this output is not connected — patch these channels
  somewhere else"); a patch past the device keeps its dimmed socket and
  says `no jack`; a device error is a `role="alert"` line; a muted or
  turned-down output gets the amber *Silent at the output:* lead, not red,
  because nothing failed. An output has **no meter**, so that amber line is
  the only place in the console a dead PA is explainable.
- Three empty states, never merged: no output device at all; a backend with
  no patchable outputs (the monitor still plays); and an output device
  present but exposing no patchable channels.
- **Patching stops the tape; re-tapping does not.** Re-pointing a jack
  costs a graph recompile, and a recompile clears FX tails into the take —
  so it is refused while recording, with the reason beside it. The pre/post
  switch is a flag write and stays live: it is the one gesture a monitor
  engineer needs mid-song.

#### MIDI rows — the same room, a different rule

MIDI outputs live in the **same modal**, below a divider that states the
rule that changes rather than merely marking that something did. They are
not a second room: a user asking "where does this go out" should be asked
one question, not made to guess which door.

- **Rows, not tiles.** An audio output holds one feed because two would
  have to be summed; a MIDI port takes any number, because events
  interleave. That is the **merge**, and it is why the MIDI half grows
  downward as a list while the audio half stays a fixed grid of jacks.
- **A `MIDI` chip where the audio sections print `OUT`**, and a **5-pin DIN**
  glyph against the quarter-inch plug and the three-pin XLR socket. Anyone
  standing at the back of the box tells the two apart by connector, so the
  console does too.
- **Their own source list, and never a mixer channel.** Three answers, each
  spelling its kind out — `Rhodes (echo)`, `nanoKEY2 (thru)`,
  `Kit Kick (take)`. The suffix is load-bearing: `nanoKEY2` alone would not
  say whether it means the keyboard playing now or a take recorded from it,
  and those reach the same jack sounding different. Offering a strip here
  would be offering a conversion the box cannot do.
- **Channel is pass-through or forced**, printed one-based (`Ch 10`) as every
  synth prints it, with a live consequence line.
- **The tap switch renders disabled with its reason**, never hidden:
  *"pre/post is a level tap — MIDI carries notes, not a signal to tap"*.
  "Why does the vocal have a pre/post switch and the Juno not" is a fair
  question with a real answer, and a control that vanishes teaches nothing.
- **A route's identity is (port, source); the channel is what you edit.**
  So re-pointing a row's source is not an edit — it is a different route,
  and the room performs it as unroute-then-route, stopping if the first
  fails.
- **A MIDI port has no meter, so it gets a counter.** `nothing sent yet` is
  the load-bearing state: without it a dead cable and a quiet keyboard look
  identical. Refused writes print first, as an alert.
- **Both ends can be missing, and they are different problems.** A route to
  an absent port says the port is not connected; a thru whose keyboard is
  unplugged blames the keyboard by name. Saying the wrong one sends somebody
  to check the wrong cable.
- **MIDI routes stay editable while the tape rolls** — the one place this
  room's recording lock does not apply, because no graph swap is involved
  and so there is no FX tail to cut. The divider says so, which is what
  makes the audio half's refusal legible instead of arbitrary.

### ChannelStrip layout (top→bottom)
INPUT · GAIN knob (red cap) · EQ section (HF blue, swept MID blue + freq
green, LF blue) · AUX sends (yellow) · PAN (white) · LedMeter · Fader (input
strips white/grey cap, bus strips blue, master red) · PFL/MUTE/ARM row ·
TapeLabel. One `--s2` gutter between blocks; `--border-panel` divider between
strips, then OUT (the output patch bay door) above the tape. Bus strips:
no GAIN, no ARM; INPUT reads "SOURCES".
- Top-right corner: the REMOVE control (`✕`, `--text-dim`, hover red).
  Destructive = two clicks: the first arms it ("SURE?", red, 3 s timeout),
  the second removes the strip. Disabled while recording (layout frozen).

### MasterSection (fixed right panel)
*(The 12-LED meters are fed one reading on both columns today; true stereo
metering needs a daemon-side `MeterKey` for each side and is not built.)*
Session TapeLabel (editable — renames the open session) · transport
(REC/STOP, **ARM ALL**, elapsed in mono) · 12-LED stereo meters · master
fader · OUT (the master mix's own patch bay door) · FX RETURN / MONITOR /
PHONES knobs. `--master-w` wide,
`--surface-section`, left `--border-panel` edge.

### ViewPager — Console ↔ Tracks
- The two rooms stack SPATIALLY: Tracks above, Console below. A slide
  transition (`--dur-3`) moves between them; both stay mounted so the
  console's live feeds never pause. The inactive page is `inert`. Note the
  limit of that promise: a full-page tab (Setup, Instruments, Bench) renders
  INSTEAD of the pager, so both rooms unmount while one is open — the stores
  survive, the components do not.
- Edge pills navigate: "View Tracks ⌃" top-center of the console,
  "View Console ⌄" bottom-center of Tracks — chevrons point where you'll go.
- Touch: pull DOWN on the console to reveal Tracks (shade-style), push UP
  on Tracks to return. Swipes need ≥110px travel with 2× vertical
  dominance (`app/swipe.ts`) so strip scrolling never pages.
- The Tracks room is the multitrack editor (below).

### Tracks editor — TakeButton · TransportBar · Timeline · Lane · TimeRuler · Playhead
- **TakeButton**: prints the selected take ("TAKE T03") in the header and
  opens the browser. An amber OLD chip appears when the selection is not
  the newest take — reviewing history rather than the last thing you cut
  is otherwise invisible. A word, not a colour.
- **TakeBrowserModal**: every take of the open session as a
  `role="listbox"` — `T03` · time of day · duration · track count, with a
  DAMAGED chip and, for a take cut at another rate, the reason PLAY will
  not roll it ("cut at 44.1 kHz · the engine runs 48 kHz"). Such a take
  stays *selectable*: refusing to show it would make it indistinguishable
  from a take that is not there. Rows disable while tape rolls, with the
  reason stated once. Delete lives in the footer and acts on the selected
  take only — a two-step confirm rather than a per-row ✕ that a thumb can
  find by accident, and no nested `<dialog>` (fiddly on iOS Safari).
  A modal, not a panel: vertical space in this view belongs to waveform.
- **TransportBar**: the tape-deck row — RTZ (ActionButton), PLAY (pfl
  PushButton, latched by server state), STOP (ActionButton, stops whatever
  runs), REC (arm PushButton, blinks while recording), the mono counter
  (`T## position / total`, `--depth-inset`; sample-derived `REC m:ss.t` in
  red while recording), a "Loop ✕" clear button while a region is set, and
  **MON** (plain PushButton): latched = the playback mix streams to THIS
  browser (AudioWorklet) while the hardware keeps the live mix; unlatched =
  playback replaces the hardware output. An "Enable audio" ActionButton
  appears only while the autoplay policy holds the AudioContext suspended.
  Buttons disable rather than hide when meaningless (no take, recording).
- **Timeline** is a VIRTUAL viewport: an absolute spacer div gives the
  scroll container its width; a `position: sticky; left: 0` pane holds
  viewport-sized canvases redrawn from `scrollLeft`. Long takes never meet
  a canvas size limit; pan is native scroll (wheel stays scroll).
- Zoom: −/+/Fit ActionButtons in the corner cell, ×2 per step, clamped
  between fit-the-take and `MIN_FPP` (32 frames/px), anchored on the view
  center. All viewport math is pure `timeline.ts`.
- **Lane**: `--lane-h` row = header column (`--lane-header-w`, strip name
  via `strip_id` with filename fallback, master = `--surface-section`,
  DAMAGED chip when samples were silence-padded, a green MIDI chip when a
  sidecar was recorded beside the track, playback SOLO — green
  `solo` PushButton variant — and MUTE) + waveform canvas
  (`--surface-inset`). Waveform = per-px min/max columns
  (`waveform-path.ts`) in `--wave-ink` over a `--wave-ink-dim` center
  line; colors reach the canvas via `getComputedStyle`, never literals.
  Lanes are POSITIONAL — keyed by index, never by (collidable) file names.
  While recording, lanes grow from live writer bins (~100 ms redraw
  throttle) and the record head rides the last bin edge.
- **TimeRuler**: canvas ticks at nice steps (≥80 px apart, `tickStepSecs`),
  mono labels; click = seek, drag = paint the loop region (`--loop-region`
  shade, `--playhead-color` edges; `loop-drag.ts` classifies at 5 px).
- **Playhead**: a 2px `--playhead-color` line spanning ruler + lanes,
  moved by rAF OUTSIDE React (`playhead-interp.ts` interpolates between
  20 Hz daemon positions; never wall-clock-driven while stopped). No
  transitions — it snaps like the LEDs. Auto-follow (`follow.ts`) pages
  the view at the 90% edge and re-centers on seeks; a manual scroll
  disarms it, PLAY or a seek re-arms.
- Peaks arrive as the daemon's binary document (`peaks-parse.ts`); the
  take document (pairs + track metadata) lives in `state/peaks.ts`.

### SetupView — recording setup

The console's rear panel: a third top-level tab rendered INSTEAD of the
ViewPager (the Bench pattern — lazy, unmounts when left; the pager's two
live views stay untouched). One centered column (max 680px) of always-open
panels: SESSION · DESTINATION · FILE FORMAT · SAMPLE RATE. Every panel is a
`--surface-strip` card with the print header row; nothing here folds —
drive status must never be hidden.

- **Session**: the reels of tape on this drive, newest first, as a
  `role="listbox"` of rows. Each row is a `TapeLabel` (the rename
  affordance — double-click, Enter or F2) plus a mono take count, Open and
  Delete. The open row is latched `aria-selected` with `--depth-inset` AND
  carries the word "open": colour is never the sole signal. A blocked Open
  or Delete renders disabled with its reason beside it ("open another
  session first", "stop recording first") — a control that vanishes
  teaches nothing.
  - **New session…** opens a dialog: a name field and a `SegmentedControl`
    for what the desk starts from — **Clean desk** / **Same channels** /
    **Copy this desk**, defaulting to Same channels, because on an
    appliance you are usually starting the next song on a rig you already
    wired. A live consequence line under the control says what resets
    ("8 channels keep their inputs and arming; EQ, sends and levels
    reset"), and a short warning appears only for the two seeds that
    actually replace the console. A confirmation that fires when nothing
    will change is the kind that gets tapped through without reading.
  - **Delete** uses the typed-word gate, not a two-click SURE?. Same
    sentence as the drive format: a strip costs nothing to rebuild,
    someone's recordings do not.
  - Two empty states, never merged: "No sessions on this drive yet" versus
    "the recording drive is not connected — its sessions cannot be
    listed".

- **Destination**: one section per physical drive, each with a print
  header (drive name + mono "57.8 GB · USB") over a `role="listbox"` grid
  of volume tiles. Internal (default root, FolderOpen) is always the first
  section; every attached drive follows, removable first, then by usable
  capacity — a drive with a ready 58 GB volume outranks one holding only
  junk. Grouping by drive is what stops a 512 MB boot partition sitting as
  an equal peer to a real stick; it is **not** a disclosure. Every volume
  of every drive is rendered, always.
  - A tile carries icon, label, and either mono "14.2 GB free" (ready) or
    "537 MB · vfat" — free space on a volume you cannot write to is a
    meaningless number. Then a status line: green LED + `ready`, or a dim
    LED and the daemon's own reason in words (`too small to record onto`,
    `connected, but nothing mounted it`, `mounted, but the recorder cannot
    write to it`, `system disk — the appliance runs from it`). The reason
    is the signal; the LED only agrees with it.
  - Unusable tiles are `disabled` and de-emphasised, never removed. A
    drive with no usable volume gets a red headline — "no volume on this
    drive can be recorded to" — and Format as the obvious next step.
  - **Format…** `ActionButton` per drive. Blocked reasons render the
    button disabled with the reason beside it, never hidden: `stop
    recording first`, `only removable drives can be formatted`, `not
    available on this installation`. It opens the wipe `Modal`, which
    prints NOW (every existing volume) against AFTER (`1 partition ·
    exFAT · TRIBUTARY · 57.8 GB`), takes a drive name (≤11 chars, exFAT's
    limit), and gates the confirm button behind the typed word `ERASE`.
    Two-click SURE? is deliberately *not* enough here — a strip costs
    nothing to rebuild, someone's recordings do not.
  - Selecting a volume records to `<mount>/tributary`. Exactly one tile
    lights (`aria-selected` + focus ring), by longest-path containment.
  - Empty states are two different sentences, never one: "No USB drive
    connected…" vs "A drive is connected, but nothing on it can be
    recorded to." A connected drive that renders as an empty port is the
    bug this panel exists to not have.
  - Below: a Custom `TextField` (absolute path) + Rescan `ActionButton`,
    and a REC PATH inset readout (mono, tail-preserving ellipsis) with its
    own lamp — red `missing` when the daemon booted on the fallback
    because the configured drive was gone.
  - The list arrives on the `destinations` WS channel: the daemon watches
    `/proc/self/mountinfo` for `POLLPRI` and pushes, so a drive plugged in
    while Setup is open appears on its own. Still no polling on either
    side — Rescan is now the manual belt, not the only path.
- **File format** (`SegmentedControl`): WAV 16 / WAV 24 / WAV 32F /
  FLAC 16 / FLAC 24 — applies to the next recording.
- **Sample rate** (`SegmentedControl`): 44.1 / 48 / 96 kHz + an ENGINE
  inset readout. Restart-gated: an amber RESTART REQUIRED lamp appears in
  the panel header whenever the configured rate differs from the running
  engine (daemon's verdict wins over the local compare).
- **Apply semantics**: immediate-apply per control (the app-wide idiom),
  but awaited: optimistic store update → PUT → confirm from the body, or
  roll back to the last server-acked state and print an inline error line
  (red LED + message, `role="status"`) inside the panel that failed.
  409 = "locked while recording", 422 = the daemon's detail verbatim,
  network = "daemon unreachable". Errors clear on the next good save.
- **Locked while recording**: the destination tiles + custom field disable
  with an amber hint line; format and rate stay live (they bind at the
  NEXT record start). The daemon enforces the same rule with a 409.

### InstrumentsView — the rack

The other rear-panel room, and a sibling of SetupView in every structural
way: a third top-level tab rendered INSTEAD of the ViewPager, one centered
680px column of always-open `Panel` cards, immediate-apply-but-awaited per
control, refusals printed inline.

Panel order is **INSTRUMENT RACK · MIDI INPUTS · SOUNDFONTS** — frequency
first, like Session at the top of Setup, and it gives the page a downward
repair gradient: a rack unit that says "no soundfont chosen" sends you to
the panel below it, one that says nothing is arriving sends you to the one
below that. Reading order is repair order.

- **A rack unit** is a `--surface-section` slab: `TapeLabel` name (the
  user's word, so it goes on tape) · `INST n` slot chip · status lamp ·
  two-click `SURE?` remove. Then rows of machine text — soundfont,
  preset, MIDI in + channel, outputs, voices — all `SelectField` or
  `SegmentedControl`, never tape. Filenames, presets and paths are machine
  text by MASTER's own rule.
- **Locked while recording is per control, matching the daemon's 409s
  exactly.** Soundfont, outputs, voices, Add and Remove disable with
  `stop recording first` beside them; preset, MIDI input and channel stay
  live, because changing sound mid-take is normal playing and a keyboardist
  on the wrong channel has to be able to fix it. The rule a user can hold:
  *changing the sound stops the tape, changing the patch doesn't.*
- **Outputs** is a two-option `SegmentedControl` — Stereo mix / Drum splits
  — with a live consequence line under it saying what the desk will get
  ("6 mono channels: Kick, Snare, Toms, HiHat, Cymbals, Percussion").
- **Add all channels to mixer** is the setup move: one named, patched strip
  per output, in one press. Adds only what is missing, so a second press
  costs nothing and levels already set survive. Its blocked reasons print
  beside it — `stop recording first`, `the console is full`.
- **Test note** sits beside it: the cheap half of "why is this silent?",
  bisecting a dead keyboard from a dead instrument with no live MIDI at
  all.
- **PANIC sits beside Refresh on the rack panel, and covers both
  directions** — it clears every internal voice *and* sends every MIDI
  output sustain-off **then** all-notes-off, in that order. One button, not
  two: once the box drives external gear, a hanging note has two possible
  homes and the user should not have to guess which. The order is not
  cosmetic — a synth holding CC 64 keeps sounding straight through CC 123,
  so all-notes-off alone would look like a broken panic button.
- The read-only tie-back line names where assignment lives:
  `feeding Kick · Snare`, or `not patched — patch it from a channel's
  INPUT button`.
- **Two empty states, never merged**: "No instruments yet — add one above."
  versus "No soundfonts on this appliance — load one below before adding an
  instrument."
- **Soundfonts** group by source with a visible header — Internal first,
  then one per drive — and every file is always rendered. A file on
  somebody's stick is theirs: its Remove is disabled with `lives on
  "STICK" — remove it there`, and the other blocked reasons (`in use by
  Rhodes`, `stop recording first`) print the same way.
- **Upload** uses `XMLHttpRequest`, and that is a deliberate deviation from
  the app's `$api` client: `xhr.upload.onprogress` is the only progress
  source that works on a plain-HTTP origin, and the appliance's origin is
  permanently insecure-context. A streamed `fetch` body requires a secure
  context, so "modernising" it would silently lose both progress and
  cancellation. Progress is honestly two-phase — byte counts while sending,
  then **"Checking the file…"** while the daemon writes and parses, because
  a bar frozen at 100% is the moment people decide the box has hung.

## Interaction + accessibility rules

1. Every control keyboard-operable. Tab order: within a strip top→bottom,
   strips left→right, master last.
2. One global `:focus-visible` treatment (`--focus-ring`); never removed.
3. Drags: pointer capture + `touch-action: none`; Shift = fine everywhere;
   double-click = reset everywhere; no wheel-to-adjust (scroll must stay
   scroll); no pointer lock.
4. All transitions ≤300ms; LED/meter changes never transition;
   `prefers-reduced-motion` collapses motion and downgrades blinks.
5. Color never the sole signal: latched buttons depress, clip prints text,
   muted strips dim their fader cap.
6. Text at `--text-2xs`/`--text-xs` is print/legend only — never interactive
   content, and always ≥ `--text-dim` contrast on its surface.

## Do / Don't

- **Do** put every new color/size/motion fact in `tokens.css` first.
- **Do** pair interactive components with a pure logic module + tests.
- **Don't** import primitives (`--gray-*`, `--cap-*`) in components — use the
  semantic role; add one if it's missing.
- **Don't** add gradients beyond the depth recipes and the two control faces
  (`--knob-body`, `--fader-cap-face`).
- **Don't** animate meters, or "smooth" LED response — the snap is the design.
- **Don't** use emoji as icons; icons are Lucide SVGs sized to the print
  scale.
