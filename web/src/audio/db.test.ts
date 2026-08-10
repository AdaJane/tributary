import { describe, expect, it } from 'vitest';

import { FADER_MIN_DB, clamp, formatDb } from './db';

describe('formatDb', () => {
  it('signs positive values and keeps one decimal', () => {
    expect(formatDb(2.5)).toBe('+2.5');
    expect(formatDb(0)).toBe('0.0');
    expect(formatDb(-6)).toBe('-6.0');
  });

  it('renders the wire floor as -∞', () => {
    expect(formatDb(FADER_MIN_DB)).toBe('-∞');
    expect(formatDb(-120)).toBe('-∞');
  });
});

describe('clamp', () => {
  it('bounds both ends', () => {
    expect(clamp(5, 0, 1)).toBe(1);
    expect(clamp(-5, 0, 1)).toBe(0);
    expect(clamp(0.5, 0, 1)).toBe(0.5);
  });
});
