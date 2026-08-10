import { describe, expect, it } from 'vitest';

import {
  MID_FREQ_MAX_HZ,
  MID_FREQ_MIN_HZ,
  formatHz,
  hzToPosition,
  positionToHz,
} from './freq';

describe('mid sweep', () => {
  it('spans the printed range', () => {
    expect(positionToHz(0)).toBe(MID_FREQ_MIN_HZ);
    expect(positionToHz(1)).toBeCloseTo(MID_FREQ_MAX_HZ, 6);
  });

  it('the midpoint is the geometric mean, not the arithmetic one', () => {
    expect(positionToHz(0.5)).toBeCloseTo(
      Math.sqrt(MID_FREQ_MIN_HZ * MID_FREQ_MAX_HZ),
      3,
    );
  });

  it('round-trips', () => {
    for (const hz of [100, 250, 800, 1200, 3500, 8000]) {
      expect(positionToHz(hzToPosition(hz))).toBeCloseTo(hz, 3);
    }
  });
});

describe('formatHz', () => {
  it('prints console-style labels', () => {
    expect(formatHz(800)).toBe('800');
    expect(formatHz(1200)).toBe('1.2k');
    expect(formatHz(8000)).toBe('8.0k');
  });
});
