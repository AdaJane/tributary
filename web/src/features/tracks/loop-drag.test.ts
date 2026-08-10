import { describe, expect, it } from 'vitest';

import { rulerGesture } from './loop-drag';

describe('rulerGesture', () => {
  it('a short press is a seek at the press point', () => {
    expect(rulerGesture(100, 103, 480, 0, 480_000)).toEqual({ kind: 'seek', frame: 48_000 });
  });

  it('a drag paints a loop, sorted regardless of direction', () => {
    expect(rulerGesture(200, 100, 480, 0, 480_000)).toEqual({
      kind: 'loop',
      startFrames: 48_000,
      endFrames: 96_000,
    });
  });

  it('honors the scroll offset and clamps to the take', () => {
    const g = rulerGesture(0, 10_000, 100, 40_000, 480_000);
    expect(g).toEqual({ kind: 'loop', startFrames: 40_000, endFrames: 480_000 });
  });
});
