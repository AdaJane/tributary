/**
 * Ruler pointer gestures: a click seeks, a drag paints a loop region.
 * Pure classification so the ruler component stays a thin shell.
 */

/** Under this much travel a press is a click, not a drag. */
export const DRAG_THRESHOLD_PX = 5;

export type RulerGesture =
  | { kind: 'seek'; frame: number }
  | { kind: 'loop'; startFrames: number; endFrames: number };

export function rulerGesture(
  downPx: number,
  upPx: number,
  fpp: number,
  startFrame: number,
  totalFrames: number,
): RulerGesture {
  const clamp = (px: number) =>
    Math.max(0, Math.min(Math.round(startFrame + px * fpp), totalFrames));
  if (Math.abs(upPx - downPx) < DRAG_THRESHOLD_PX) {
    return { kind: 'seek', frame: clamp(downPx) };
  }
  return {
    kind: 'loop',
    startFrames: clamp(Math.min(downPx, upPx)),
    endFrames: clamp(Math.max(downPx, upPx)),
  };
}
