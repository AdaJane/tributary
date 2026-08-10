import { describe, expect, it } from 'vitest';

import { MIN_FPP, clampFpp, fitFpp, frameToPx, pxToFrame, tickStepSecs, totalPx } from './timeline';

describe('timeline', () => {
  it('fit shows the whole take across the viewport', () => {
    expect(fitFpp(480_000, 1_000)).toBe(480);
    expect(totalPx(480_000, 480)).toBe(1_000);
  });

  it('fit never zooms in past the floor for short takes', () => {
    expect(fitFpp(1_000, 1_000)).toBe(MIN_FPP);
  });

  it('clamps zoom between fit and the floor', () => {
    expect(clampFpp(10_000, 480_000, 1_000)).toBe(480);
    expect(clampFpp(1, 480_000, 1_000)).toBe(MIN_FPP);
    expect(clampFpp(100, 480_000, 1_000)).toBe(100);
  });

  it('px and frames round-trip through the zoom', () => {
    expect(frameToPx(48_000, 480)).toBe(100);
    expect(pxToFrame(100, 480)).toBe(48_000);
    expect(pxToFrame(-5, 480)).toBe(0);
  });

  it('picks tick steps that keep at least 80px between ticks', () => {
    // 480 fpp at 48k = 100px per second: 1s ticks fit.
    expect(tickStepSecs(480, 48_000)).toBe(1);
    // Deep zoom: 32 fpp = 1500px/s — 0.1s ticks are 150px apart.
    expect(tickStepSecs(32, 48_000)).toBe(0.1);
    // Far out: 10min take on a laptop → coarse ticks.
    expect(tickStepSecs(28_800_000 / 1_280, 48_000)).toBe(60);
  });
});
