/**
 * Auto-follow scroll policy: when the playhead nears the right edge, page
 * the view so it lands near the left; when it leaves the view entirely
 * (a seek), center it. Returns the new scrollLeft, or null to stay put.
 * The caller owns the armed/disarmed state (manual scroll disarms).
 */
export function followScroll(
  playheadPx: number,
  scrollLeft: number,
  viewWidthPx: number,
  contentWidthPx: number,
): number | null {
  const clamp = (v: number) => Math.max(0, Math.min(v, contentWidthPx - viewWidthPx));
  const inView = playheadPx >= scrollLeft && playheadPx <= scrollLeft + viewWidthPx;
  if (!inView) return clamp(playheadPx - viewWidthPx / 2);
  if (playheadPx > scrollLeft + viewWidthPx * 0.9) {
    return clamp(playheadPx - viewWidthPx * 0.1);
  }
  return null;
}
