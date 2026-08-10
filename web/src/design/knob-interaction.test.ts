import { describe, expect, it } from 'vitest';

import {
  DRAG_RANGE_PX,
  FINE_SCALE,
  dragPosition,
  positionToAngle,
  stepValue,
} from './knob-interaction';

describe('dragPosition', () => {
  const start = { position: 0.5, pointerY: 300 };

  it('a full-range upward drag reaches the top', () => {
    expect(dragPosition(start, 300 - DRAG_RANGE_PX / 2, false)).toBe(1);
  });

  it('downward movement decreases and clamps at zero', () => {
    expect(dragPosition(start, 300 + DRAG_RANGE_PX, false)).toBe(0);
  });

  it('shift scales the same movement down for fine adjustment', () => {
    const coarse = dragPosition(start, 285, false);
    const fine = dragPosition(start, 285, true);
    expect(coarse - start.position).toBeCloseTo(
      (fine - start.position) / FINE_SCALE,
      6,
    );
  });
});

describe('positionToAngle', () => {
  it('sweeps 270° with the detent at 12 o’clock', () => {
    expect(positionToAngle(0)).toBe(-135);
    expect(positionToAngle(0.5)).toBe(0);
    expect(positionToAngle(1)).toBe(135);
  });
});

describe('stepValue', () => {
  it('steps in domain units and clamps at the ends', () => {
    expect(stepValue(0, 1, 0.5, -90, 10)).toBe(0.5);
    expect(stepValue(9.8, 1, 0.5, -90, 10)).toBe(10);
    expect(stepValue(-89.9, -1, 3, -90, 10)).toBe(-90);
  });
});
