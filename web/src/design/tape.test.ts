import { describe, expect, it } from 'vitest';

import { TAPE_ROTATE_MAX_DEG, rotationFor } from './tape';

describe('rotationFor', () => {
  it('is deterministic per id', () => {
    expect(rotationFor('strip-3')).toBe(rotationFor('strip-3'));
  });

  it('different ids get different leans', () => {
    expect(rotationFor('strip-1')).not.toBe(rotationFor('strip-2'));
  });

  it('stays within the tape budget', () => {
    for (let i = 0; i < 50; i++) {
      const deg = rotationFor(`strip-${i}`);
      expect(Math.abs(deg)).toBeLessThanOrEqual(TAPE_ROTATE_MAX_DEG);
    }
  });
});
