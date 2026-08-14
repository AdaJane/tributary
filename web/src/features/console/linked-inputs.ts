/**
 * Input-patch domain logic: jack identity across devices, which strips
 * share one, and the color of the tape that marks the link. A jack is
 * `(device, channel)` — two devices' channel 0 are different jacks.
 */
import type { CapColor } from '../../design/Knob';
import type { StripState } from '../../ws/messages';
import { inputSource, sourceKey } from './input-source';
import type { InputSource } from './input-source';

/** Stable string identity for a jack. `device` null = system default. */
export function jackKey(device: string | null, channel: number): string {
  return sourceKey({ kind: 'device', device, channel });
}

/**
 * Stage-box letters: the system default input is always A; named devices
 * get B, C… in lexicographic order — deterministic from the name set
 * alone, no matter what order the OS lists them in.
 */
export function deviceLetters(names: readonly string[]): Map<string | null, string> {
  const letters = new Map<string | null, string>([[null, 'A']]);
  [...new Set(names)].sort().forEach((name, i) => {
    letters.set(name, String.fromCharCode('B'.charCodeAt(0) + i));
  });
  return letters;
}

/** Console-style print for a jack: `IN 3` on the default box, `B3` on
 * stage box B. Unknown devices print `?3` until enumeration names them. */
export function jackLabel(
  device: string | null,
  channel: number,
  letters: Map<string | null, string>,
): string {
  if (device === null) return `IN ${channel + 1}`;
  const letter = letters.get(device) ?? '?';
  return `${letter}${channel + 1}`;
}

/**
 * What a strip's INPUT button prints, whatever it is patched to.
 *
 * Instruments print `INST 3.1` — the rack slot and the channel within it —
 * rather than borrowing a stage-box letter. Lettering them would collide
 * (box `I` against `I1`) and, worse, adding one would re-letter the stage
 * boxes and change the colour and print of the tape on two strips under
 * the user's hands.
 */
export function sourceLabel(
  source: InputSource | null,
  letters: Map<string | null, string>,
): string {
  if (source === null) return '—';
  if (source.kind === 'instrument') {
    return `INST ${source.instrument + 1}.${source.channel + 1}`;
  }
  return jackLabel(source.device, source.channel, letters);
}

/** Stripe palette keyed by jack identity (hashed), so a link keeps its
 * color no matter which strips join or leave it. */
const STRIPE_COLORS: readonly CapColor[] = ['yellow', 'blue', 'green', 'red', 'grey', 'white'];

export function stripeColor(key: string): CapColor {
  // djb2 — tiny, stable, good enough to spread six colors.
  let hash = 5381;
  for (let i = 0; i < key.length; i += 1) {
    hash = ((hash << 5) + hash + key.charCodeAt(i)) | 0;
  }
  return STRIPE_COLORS[Math.abs(hash) % STRIPE_COLORS.length];
}

export interface InputLink {
  key: string;
  source: InputSource;
  color: CapColor;
  /** Print label for the tape ("IN 3", "B2"). */
  label: string;
  stripIds: number[];
}

/** Jacks feeding two or more strips — the ones that earn a tape stripe. */
export function sharedInputGroups(
  strips: readonly StripState[],
  letters: Map<string | null, string>,
): Map<string, InputLink> {
  const byKey = new Map<string, { source: InputSource; ids: number[] }>();
  for (const strip of strips) {
    const source = inputSource(strip.input);
    if (source === null) continue;
    const key = sourceKey(source);
    const entry = byKey.get(key) ?? { source, ids: [] };
    entry.ids.push(strip.id);
    byKey.set(key, entry);
  }
  const links = new Map<string, InputLink>();
  for (const [key, { source, ids }] of byKey) {
    if (ids.length >= 2) {
      links.set(key, {
        key,
        source,
        color: stripeColor(key),
        label: sourceLabel(source, letters),
        stripIds: ids,
      });
    }
  }
  return links;
}
