import { describe, expect, it } from 'vitest';

import { estimatePosition } from './playhead-interp';

describe('estimatePosition', () => {
  it('advances by real time at the sample rate while playing', () => {
    expect(estimatePosition(48_000, 1_000, 1_250, 48_000, true, 480_000, null)).toBe(60_000);
  });

  it('holds still when stopped', () => {
    expect(estimatePosition(48_000, 1_000, 9_999, 48_000, false, 480_000, null)).toBe(48_000);
  });

  it('never runs past the take', () => {
    expect(estimatePosition(479_000, 0, 10_000, 48_000, true, 480_000, null)).toBe(480_000);
  });

  it('wraps inside the loop region', () => {
    const loop = { startFrames: 48_000, endFrames: 96_000 };
    // 1.5s past the loop end → wraps 1.5 loop-lengths in.
    expect(estimatePosition(94_000, 0, 1_000, 48_000, true, 480_000, loop)).toBe(
      48_000 + ((94_000 + 48_000 - 48_000) % 48_000),
    );
  });

  it('ignores a clock that went backwards', () => {
    expect(estimatePosition(48_000, 2_000, 1_000, 48_000, true, 480_000, null)).toBe(48_000);
  });
});
