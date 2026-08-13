import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { FrameBatcher } from './batcher';

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('FrameBatcher', () => {
  it('coalesces per key and flushes on the interval', () => {
    const flushed: Map<string, number>[] = [];
    const batcher = new FrameBatcher<number>(50, (b) => flushed.push(b));
    batcher.push('a', 1);
    batcher.push('b', 2);
    expect(flushed).toHaveLength(0);
    vi.advanceTimersByTime(50);
    expect(flushed).toHaveLength(1);
    expect([...flushed[0]]).toEqual([
      ['a', 1],
      ['b', 2],
    ]);
    batcher.stop();
  });

  it('without a merge the last write wins', () => {
    const flushed: Map<string, number>[] = [];
    const batcher = new FrameBatcher<number>(50, (b) => flushed.push(b));
    batcher.push('a', 1);
    batcher.push('a', 2);
    vi.advanceTimersByTime(50);
    expect(flushed[0].get('a')).toBe(2);
    batcher.stop();
  });

  it('a merge decides what survives a collision', () => {
    // The meter case: two publishes inside one flush window must not lose
    // the louder peak. The daemon's own coalescer takes the max; a client
    // that takes the last would under-report every transient it split.
    const flushed: Map<string, number>[] = [];
    const batcher = new FrameBatcher<number>(
      50,
      (b) => flushed.push(b),
      (prev, next) => Math.max(prev, next),
    );
    batcher.push('a', -12);
    batcher.push('a', -40);
    vi.advanceTimersByTime(50);
    expect(flushed[0].get('a')).toBe(-12);
    batcher.stop();
  });

  it('merges only within a window, never across a flush', () => {
    const flushed: Map<string, number>[] = [];
    const batcher = new FrameBatcher<number>(
      50,
      (b) => flushed.push(b),
      (prev, next) => Math.max(prev, next),
    );
    batcher.push('a', -12);
    vi.advanceTimersByTime(50);
    batcher.push('a', -40);
    vi.advanceTimersByTime(50);
    expect(flushed.map((b) => b.get('a'))).toEqual([-12, -40]);
    batcher.stop();
  });

  it('goes idle when drained and wakes on the next push', () => {
    const flushed: Map<string, number>[] = [];
    const batcher = new FrameBatcher<number>(50, (b) => flushed.push(b));
    batcher.push('a', 1);
    vi.advanceTimersByTime(200);
    expect(flushed).toHaveLength(1);
    batcher.push('a', 2);
    vi.advanceTimersByTime(50);
    expect(flushed).toHaveLength(2);
    batcher.stop();
  });
});
