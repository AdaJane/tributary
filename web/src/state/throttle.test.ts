import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { GestureThrottle } from './throttle';

describe('GestureThrottle', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('sends immediately when idle, then defers within the window', () => {
    const sent: number[] = [];
    const throttle = new GestureThrottle<number>(30, (_k, v) => sent.push(v));
    throttle.push('fader', 1, 1000);
    throttle.push('fader', 2, 1010);
    throttle.push('fader', 3, 1020);
    expect(sent).toEqual([1]);
    vi.advanceTimersByTime(30);
    // The FINAL value went out; the middle one was dropped.
    expect(sent).toEqual([1, 3]);
  });

  it('separate keys throttle independently', () => {
    const sent: string[] = [];
    const throttle = new GestureThrottle<number>(30, (k) => sent.push(k));
    throttle.push('fader', 1, 1000);
    throttle.push('pan', 1, 1001);
    expect(sent).toEqual(['fader', 'pan']);
  });
});
