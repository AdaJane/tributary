/**
 * Pure drag/angle math shared by Knob and Fader. Components own the DOM;
 * this owns every number, so the geometry is testable without a layout.
 */
import { clamp } from '../audio/db';

/** Vertical pixels of drag for a full control sweep. */
export const DRAG_RANGE_PX = 150;
/** Shift held: gesture resolution multiplier. */
export const FINE_SCALE = 0.1;

export interface DragStart {
  /** Normalized position [0,1] when the pointer went down. */
  readonly position: number;
  readonly pointerY: number;
}

/** Position for the current pointer height. Up = increase. */
export function dragPosition(
  start: DragStart,
  pointerY: number,
  fine: boolean,
): number {
  const travel = (start.pointerY - pointerY) / DRAG_RANGE_PX;
  return clamp(start.position + travel * (fine ? FINE_SCALE : 1), 0, 1);
}

/** Knob sweep: 270°, detent (position 0.5) at 12 o'clock. */
export const SWEEP_DEG = 270;

export function positionToAngle(position: number): number {
  return -SWEEP_DEG / 2 + clamp(position, 0, 1) * SWEEP_DEG;
}

/** One keyboard step in domain units; PageUp/Down use `bigStep`. */
export function stepValue(
  value: number,
  direction: 1 | -1,
  step: number,
  min: number,
  max: number,
): number {
  return clamp(value + direction * step, min, max);
}
