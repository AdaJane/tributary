/**
 * Touch swipe detection, pure. The pager records where a touch began and
 * asks this where it went — vertical dominance keeps horizontal strip
 * scrolling from paging by accident.
 */

/** Minimum travel to count as a page swipe, not a tap or wobble. */
export const SWIPE_THRESHOLD_PX = 110;
/** Vertical travel must beat horizontal by this factor. */
export const SWIPE_AXIS_RATIO = 2;

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** 'down' = finger moved down (pulling the upper page into view). */
export function swipeDirection(start: Point, end: Point): 'up' | 'down' | null {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  if (Math.abs(dy) < SWIPE_THRESHOLD_PX) return null;
  if (Math.abs(dy) < Math.abs(dx) * SWIPE_AXIS_RATIO) return null;
  return dy > 0 ? 'down' : 'up';
}
