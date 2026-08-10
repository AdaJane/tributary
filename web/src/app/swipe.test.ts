import { describe, expect, it } from 'vitest';

import { SWIPE_THRESHOLD_PX, swipeDirection } from './swipe';

describe('swipeDirection', () => {
  it('a firm downward pull reads down, upward reads up', () => {
    expect(swipeDirection({ x: 0, y: 0 }, { x: 10, y: 200 })).toBe('down');
    expect(swipeDirection({ x: 0, y: 300 }, { x: -8, y: 80 })).toBe('up');
  });

  it('short travel is a tap, not a swipe', () => {
    expect(
      swipeDirection({ x: 0, y: 0 }, { x: 0, y: SWIPE_THRESHOLD_PX - 1 }),
    ).toBeNull();
  });

  it('diagonal drags (strip scrolling) never page', () => {
    expect(swipeDirection({ x: 0, y: 0 }, { x: 200, y: 150 })).toBeNull();
  });
});
