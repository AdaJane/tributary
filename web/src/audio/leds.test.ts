import { describe, expect, it } from 'vitest';

import {
  CHANNEL_LED_STOPS,
  CLIP_HOLD_MS,
  MASTER_LED_STOPS,
  applyClipHold,
  belowScale,
  formatPeak,
  isClipHeld,
  litSegments,
} from './leds';

describe('litSegments', () => {
  it('lights nothing below the first stop and everything at the top', () => {
    expect(litSegments(-90, CHANNEL_LED_STOPS)).toBe(0);
    expect(litSegments(0, CHANNEL_LED_STOPS)).toBe(CHANNEL_LED_STOPS.length);
  });

  it('a healthy -20 dB signal reads three green LEDs', () => {
    expect(litSegments(-20, CHANNEL_LED_STOPS)).toBe(3);
    expect(CHANNEL_LED_STOPS[2].color).toBe('green');
  });

  it('threshold values light their own LED', () => {
    expect(litSegments(-6, CHANNEL_LED_STOPS)).toBe(5);
  });

  it('master scale is denser but agrees at the top', () => {
    expect(litSegments(0, MASTER_LED_STOPS)).toBe(MASTER_LED_STOPS.length);
    expect(litSegments(-100, MASTER_LED_STOPS)).toBe(0);
  });
});

describe('calibration', () => {
  // These tables are the calibration, so they answer to the design system
  // and nothing else. A stale duplicate in trib-core disagreed with them
  // for long enough to be quoted as fact in two doc comments.
  it('the channel scale matches the LedMeter spec exactly', () => {
    expect(CHANNEL_LED_STOPS.map((s) => [s.db, s.color])).toEqual([
      [-40, 'green'],
      [-30, 'green'],
      [-20, 'green'],
      [-12, 'green'],
      [-6, 'amber'],
      [-3, 'amber'],
      [0, 'red'],
    ]);
  });

  it('the master scale is 12 per side, green to -12, amber to -3, red above', () => {
    expect(MASTER_LED_STOPS).toHaveLength(12);
    const zone = (db: number) => MASTER_LED_STOPS.find((s) => s.db === db)?.color;
    expect(zone(-12)).toBe('green');
    expect(zone(-9)).toBe('amber');
    expect(zone(-3)).toBe('amber');
    expect(zone(0)).toBe('red');
  });
});

describe('below scale', () => {
  // Zero lit LEDs cannot, on its own, tell "nothing patched" from "patched
  // and 50 dB too quiet" — the exact ambiguity that hid a dead input.
  it('separates a quiet signal from actual silence', () => {
    expect(belowScale(-50, CHANNEL_LED_STOPS)).toBe(true);
    expect(litSegments(-50, CHANNEL_LED_STOPS)).toBe(0);
  });

  it('true silence is not "below scale"', () => {
    expect(belowScale(-90, CHANNEL_LED_STOPS)).toBe(false);
  });

  it('a signal on the scale is not below it', () => {
    expect(belowScale(-40, CHANNEL_LED_STOPS)).toBe(false);
    expect(belowScale(-12, CHANNEL_LED_STOPS)).toBe(false);
  });

  it('the master scale reaches lower before it gives up', () => {
    expect(belowScale(-50, MASTER_LED_STOPS)).toBe(false);
    expect(belowScale(-70, MASTER_LED_STOPS)).toBe(true);
  });
});

describe('formatPeak', () => {
  it('names the floor rather than printing it', () => {
    expect(formatPeak(-90)).toBe('silent');
    expect(formatPeak(-200)).toBe('silent');
  });

  it('keeps the number the LEDs threw away', () => {
    expect(formatPeak(-89.4)).toBe('-89.4 dB');
    expect(formatPeak(-0.5)).toBe('-0.5 dB');
  });
});

describe('clip hold', () => {
  it('a clip frame latches for CLIP_HOLD_MS', () => {
    const heldUntil = applyClipHold(0, true, 1000);
    expect(heldUntil).toBe(1000 + CLIP_HOLD_MS);
    expect(isClipHeld(heldUntil, 1000 + CLIP_HOLD_MS - 1)).toBe(true);
    expect(isClipHeld(heldUntil, 1000 + CLIP_HOLD_MS)).toBe(false);
  });

  it('non-clip frames keep an active hold and clear an expired one', () => {
    expect(applyClipHold(5000, false, 4000)).toBe(5000);
    expect(applyClipHold(5000, false, 6000)).toBe(0);
  });

  it('a fresh clip extends an existing hold', () => {
    expect(applyClipHold(5000, true, 4500)).toBe(4500 + CLIP_HOLD_MS);
  });
});
