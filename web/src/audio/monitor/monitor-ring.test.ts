import { describe, expect, it } from 'vitest';

import { MonitorRing } from './monitor-processor';

describe('MonitorRing', () => {
  it('round-trips interleaved stereo into planar outputs', () => {
    const ring = new MonitorRing(16);
    ring.write(Float32Array.from([1, -1, 2, -2]));
    const left = new Float32Array(3);
    const right = new Float32Array(3);
    ring.readStereo(left, right);
    expect(Array.from(left)).toEqual([1, 2, 0]);
    expect(Array.from(right)).toEqual([-1, -2, 0]);
    expect(ring.available()).toBe(0);
  });

  it('drops the oldest audio on overflow', () => {
    const ring = new MonitorRing(4);
    ring.write(Float32Array.from([1, 2, 3, 4, 5, 6]));
    const left = new Float32Array(2);
    const right = new Float32Array(2);
    ring.readStereo(left, right);
    // The two oldest frames were dropped.
    expect(Array.from(left)).toEqual([3, 5]);
    expect(Array.from(right)).toEqual([4, 6]);
  });

  it('clear flushes everything (the generation change)', () => {
    const ring = new MonitorRing(8);
    ring.write(Float32Array.from([1, 2, 3, 4]));
    ring.clear();
    expect(ring.available()).toBe(0);
    const left = new Float32Array(1);
    const right = new Float32Array(1);
    ring.readStereo(left, right);
    expect(left[0]).toBe(0);
  });

  it('wraps correctly across many writes and reads', () => {
    const ring = new MonitorRing(6);
    for (let pass = 0; pass < 5; pass += 1) {
      ring.write(Float32Array.from([pass, -pass, pass + 10, -(pass + 10)]));
      const left = new Float32Array(2);
      const right = new Float32Array(2);
      ring.readStereo(left, right);
      expect(Array.from(left)).toEqual([pass, pass + 10]);
      expect(Array.from(right)).toEqual([-pass, -(pass + 10)]);
    }
  });
});
