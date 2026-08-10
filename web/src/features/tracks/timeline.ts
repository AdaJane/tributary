/**
 * Timeline viewport math. One number rules the view: `fpp`, frames per
 * CSS pixel. Zooming halves/doubles it; fit derives it from the take.
 */

/** Deepest zoom-in: 32 frames per pixel (bins stretch below this). */
export const MIN_FPP = 32;

/** Fixed zoom while recording — fit would rescale as the take grows. */
export const RECORD_FPP = 256;

/** Zoom step factor for the +/- controls. */
export const ZOOM_STEP = 2;

/** Frames per pixel that fits the whole take in the viewport. */
export function fitFpp(totalFrames: number, viewWidthPx: number): number {
  if (totalFrames <= 0 || viewWidthPx <= 0) return MIN_FPP;
  return Math.max(MIN_FPP, totalFrames / viewWidthPx);
}

/** Legal zoom range: between full-take fit and MIN_FPP. */
export function clampFpp(fpp: number, totalFrames: number, viewWidthPx: number): number {
  return Math.min(Math.max(fpp, MIN_FPP), fitFpp(totalFrames, viewWidthPx));
}

export function frameToPx(frame: number, fpp: number): number {
  return frame / fpp;
}

export function pxToFrame(px: number, fpp: number): number {
  return Math.max(0, Math.round(px * fpp));
}

/** Content width of the whole take at this zoom. */
export function totalPx(totalFrames: number, fpp: number): number {
  return Math.ceil(totalFrames / fpp);
}

const TICK_STEPS_SECS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];

/** Ruler tick spacing in seconds: the smallest nice step ≥ ~80px apart. */
export function tickStepSecs(fpp: number, sampleRate: number): number {
  const minPx = 80;
  for (const step of TICK_STEPS_SECS) {
    if ((step * sampleRate) / fpp >= minPx) return step;
  }
  return TICK_STEPS_SECS[TICK_STEPS_SECS.length - 1];
}
