import { describe, expect, it } from 'vitest';

import {
  CHANNEL_LED_STOPS,
  CLIP_HOLD_MS,
  MASTER_LED_STOPS,
  applyClipHold,
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
