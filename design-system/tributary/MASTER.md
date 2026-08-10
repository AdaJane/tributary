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
LED (`--led-size` 8px), buttons (`--btn-*`), tape (`--tape-*`), strip/master
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
  above. Thresholds live in `audio/leds.ts` (mirrors `trib-core` constants).
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
  default box draws the classic 8 jacks.
- An in-use jack is still selectable — sharing is legal, and is exactly
  what the tape stripe marks. Footer: hint text + Refresh + Disconnect
  (disabled when unpatched).

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

### ChannelStrip layout (top→bottom)
INPUT · GAIN knob (red cap) · EQ section (HF blue, swept MID blue + freq
green, LF blue) · AUX sends (yellow) · PAN (white) · LedMeter · Fader (input
strips white/grey cap, bus strips blue, master red) · PFL/MUTE/ARM row ·
TapeLabel. One `--s2` gutter between blocks; `--border-panel` divider between
strips. Bus strips: no GAIN, no ARM; INPUT reads "SOURCES".
- Top-right corner: the REMOVE control (`✕`, `--text-dim`, hover red).
  Destructive = two clicks: the first arms it ("SURE?", red, 3 s timeout),
  the second removes the strip. Disabled while recording (layout frozen).

### MasterSection (fixed right panel)
Project TapeLabel (editable) · transport (REC/STOP, **ARM ALL**, elapsed in
mono) · 12-LED stereo meters · master fader · FX RETURN / MONITOR / PHONES
knobs. `--master-w` wide, `--surface-section`, left `--border-panel` edge.

### ViewPager — Console ↔ Tracks
- The two rooms stack SPATIALLY: Tracks above, Console below. A slide
  transition (`--dur-3`) moves between them; both stay mounted so the
  console's live feeds never pause. The inactive page is `inert`.
- Edge pills navigate: "View Tracks ⌃" top-center of the console,
  "View Console ⌄" bottom-center of Tracks — chevrons point where you'll go.
- Touch: pull DOWN on the console to reveal Tracks (shade-style), push UP
  on Tracks to return. Swipes need ≥110px travel with 2× vertical
  dominance (`app/swipe.ts`) so strip scrolling never pages.
- The Tracks room is the multitrack editor (below).

### Tracks editor — TransportBar · Timeline · Lane · TimeRuler · Playhead
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
  DAMAGED chip when samples were silence-padded, playback SOLO — green
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
panels: DESTINATION · FILE FORMAT · SAMPLE RATE. Every panel is a
`--surface-strip` card with the print header row; nothing here folds —
drive status must never be hidden.

- **Destination**: a `role="listbox"` grid of drive tiles — Internal
  (default root, FolderOpen) first, then mounted drives (Usb/HardDrive
  icon, label, mono "14.2 GB free", status word + LED: green `ready`,
  amber `read-only`). Selecting a drive records to `<mount>/tributary`.
  Exactly one tile lights (`aria-selected` + focus ring), chosen by
  longest-path containment. Below: a Custom `TextField` (absolute path)
  + Rescan `ActionButton` (manual only — the daemon never polls), and a
  REC PATH inset readout (mono, tail-preserving ellipsis) with its own
  lamp — red `missing` when the daemon booted on the fallback because the
  configured drive was gone.
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
