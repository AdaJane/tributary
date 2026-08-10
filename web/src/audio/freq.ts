/**
 * The swept-mid frequency knob: logarithmic over the sweep range. The range
 * mirrors trib-core's `eq.rs`.
 */
import { clamp } from './db';

export const MID_FREQ_MIN_HZ = 100;
export const MID_FREQ_MAX_HZ = 8000;

const RATIO = MID_FREQ_MAX_HZ / MID_FREQ_MIN_HZ;

export function positionToHz(position: number): number {
  return MID_FREQ_MIN_HZ * RATIO ** clamp(position, 0, 1);
}

export function hzToPosition(hz: number): number {
  const f = clamp(hz, MID_FREQ_MIN_HZ, MID_FREQ_MAX_HZ);
  return Math.log(f / MID_FREQ_MIN_HZ) / Math.log(RATIO);
}

/** Console-style print: "800", "1.2k", "8.0k". */
export function formatHz(hz: number): string {
  return hz >= 1000 ? `${(hz / 1000).toFixed(1)}k` : `${Math.round(hz)}`;
}
