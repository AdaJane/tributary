/**
 * LED semantics. These tables ARE the calibration — the daemon reports dB
 * and owns only the clip threshold; where the zones fall is presentation,
 * and differs between the channel and master meters. The spec they answer
 * to is `LedMeter` in design-system/tributary/MASTER.md.
 */
import { FADER_MIN_DB, formatDb } from './db';

export type LedColor = 'green' | 'amber' | 'red';

export interface LedStop {
  readonly db: number;
  readonly color: LedColor;
}

/** Channel strips: 7 LEDs, bottom-up. */
export const CHANNEL_LED_STOPS: readonly LedStop[] = [
  { db: -40, color: 'green' },
  { db: -30, color: 'green' },
  { db: -20, color: 'green' },
  { db: -12, color: 'green' },
  { db: -6, color: 'amber' },
  { db: -3, color: 'amber' },
  { db: 0, color: 'red' },
];

/** Master: 12 LEDs per side, bottom-up. */
export const MASTER_LED_STOPS: readonly LedStop[] = [
  { db: -57, color: 'green' },
  { db: -48, color: 'green' },
  { db: -42, color: 'green' },
  { db: -36, color: 'green' },
  { db: -30, color: 'green' },
  { db: -24, color: 'green' },
  { db: -18, color: 'green' },
  { db: -12, color: 'green' },
  { db: -9, color: 'amber' },
  { db: -6, color: 'amber' },
  { db: -3, color: 'amber' },
  { db: 0, color: 'red' },
];

/** How many LEDs light for a peak — the quantizing selector's output, so a
 * strip re-renders only when this integer changes. */
export function litSegments(peakDb: number, stops: readonly LedStop[]): number {
  let lit = 0;
  for (const stop of stops) {
    if (peakDb >= stop.db) lit++;
    else break;
  }
  return lit;
}

/** A real signal that lands under the bottom stop.
 *
 * Zero lit LEDs is otherwise ambiguous: nothing patched, a muted source,
 * and a correctly patched input sitting 50 dB below the scale all look
 * identical. They are very different problems, and only this tells them
 * apart on the console itself. */
export function belowScale(peakDb: number, stops: readonly LedStop[]): boolean {
  return peakDb > FADER_MIN_DB && peakDb < stops[0].db;
}

/** The reading in words, for a tooltip and for `aria-valuetext` — the LEDs
 * are quantized to the scale, so this is the only place the actual number
 * survives. */
export function formatPeak(peakDb: number): string {
  return peakDb <= FADER_MIN_DB ? 'silent' : `${formatDb(peakDb)} dB`;
}

/** Clip latch duration. Detection is the daemon's (it saw every sample);
 * the hold is presentation, so it lives client-side. */
export const CLIP_HOLD_MS = 1800;

/** Returns the new hold-expiry timestamp (0 = no hold). */
export function applyClipHold(
  heldUntil: number,
  frameClip: boolean,
  now: number,
): number {
  if (frameClip) return now + CLIP_HOLD_MS;
  return heldUntil > now ? heldUntil : 0;
}

export function isClipHeld(heldUntil: number, now: number): boolean {
  return heldUntil > now;
}
