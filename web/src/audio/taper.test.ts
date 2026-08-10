import { describe, expect, it } from 'vitest';

import { FADER_MIN_DB } from './db';
import { dbToPosition, positionToDb } from './taper';

describe('fader taper', () => {
  it('hits the printed anchors', () => {
    expect(positionToDb(1)).toBe(10);
    expect(positionToDb(0.75)).toBe(0);
    expect(positionToDb(0.5)).toBe(-10);
    expect(positionToDb(0)).toBe(FADER_MIN_DB);
  });

  it('unity sits at 75% travel', () => {
    expect(dbToPosition(0)).toBe(0.75);
  });

  it('round-trips position through dB and back', () => {
    for (let t = 0; t <= 1.001; t += 0.05) {
      expect(dbToPosition(positionToDb(t))).toBeCloseTo(Math.min(t, 1), 5);
    }
  });

  it('clamps out-of-range input instead of extrapolating', () => {
    expect(positionToDb(1.5)).toBe(10);
    expect(positionToDb(-0.5)).toBe(FADER_MIN_DB);
    expect(dbToPosition(99)).toBe(1);
    expect(dbToPosition(-200)).toBe(0);
  });

  it('is monotonic', () => {
    let last = -Infinity;
    for (let t = 0; t <= 1.001; t += 0.01) {
      const db = positionToDb(t);
      expect(db).toBeGreaterThanOrEqual(last);
      last = db;
    }
  });
});
