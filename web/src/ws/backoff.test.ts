import { describe, expect, it } from 'vitest';

import { BACKOFF_MAX_MS, backoffDelay } from './backoff';

describe('backoffDelay', () => {
  it('grows exponentially and caps', () => {
    const mid = () => 0.5;
    expect(backoffDelay(0, mid)).toBe(750);
    expect(backoffDelay(1, mid)).toBe(1500);
    expect(backoffDelay(20, mid)).toBe(BACKOFF_MAX_MS * 0.75);
  });

  it('jitter keeps delays within 50-100% of the window', () => {
    expect(backoffDelay(2, () => 0)).toBe(2000);
    expect(backoffDelay(2, () => 1)).toBe(4000);
  });
});
