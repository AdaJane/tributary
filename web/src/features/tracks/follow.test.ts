import { describe, expect, it } from 'vitest';

import { followScroll } from './follow';

describe('followScroll', () => {
  it('stays put while the playhead is comfortably visible', () => {
    expect(followScroll(500, 0, 1_000, 5_000)).toBeNull();
  });

  it('pages forward when the playhead crosses 90% of the view', () => {
    expect(followScroll(950, 0, 1_000, 5_000)).toBe(850);
  });

  it('centers on a jump out of view (a seek)', () => {
    expect(followScroll(3_000, 0, 1_000, 5_000)).toBe(2_500);
  });

  it('never scrolls past the content edges', () => {
    expect(followScroll(4_990, 3_000, 1_000, 5_000)).toBe(4_000);
    expect(followScroll(10, 2_000, 1_000, 5_000)).toBe(0);
  });
});
