/**
 * Live meter readings, fed by the 50 ms FrameBatcher. Components subscribe
 * with quantizing selectors (litSegments) so a strip re-renders only when
 * its LED count actually changes.
 */
import { create } from 'zustand';

import { applyClipHold, isClipHeld } from '../audio/leds';
import { FADER_MIN_DB } from '../audio/db';

export interface MeterReading {
  peakDb: number;
  /** Clip-hold expiry timestamp; 0 = not held. */
  clipHeldUntil: number;
}

export const SILENT_READING: MeterReading = {
  peakDb: FADER_MIN_DB,
  clipHeldUntil: 0,
};

interface MetersStore {
  byKey: Record<string, MeterReading>;
  applyBatch: (
    batch: Map<string, { peakDb: number; clip: boolean }>,
    now: number,
  ) => void;
  clearClip: (key: string) => void;
}

export const useMeters = create<MetersStore>((set) => ({
  byKey: {},
  applyBatch: (batch, now) =>
    set((s) => {
      const byKey = { ...s.byKey };
      for (const [key, frame] of batch) {
        const prev = byKey[key] ?? SILENT_READING;
        byKey[key] = {
          peakDb: frame.peakDb,
          clipHeldUntil: applyClipHold(prev.clipHeldUntil, frame.clip, now),
        };
      }
      return { byKey };
    }),
  clearClip: (key) =>
    set((s) => {
      const prev = s.byKey[key];
      if (!prev) return s;
      return { byKey: { ...s.byKey, [key]: { ...prev, clipHeldUntil: 0 } } };
    }),
}));

export function readingClip(reading: MeterReading, now: number): boolean {
  return isClipHeld(reading.clipHeldUntil, now);
}
