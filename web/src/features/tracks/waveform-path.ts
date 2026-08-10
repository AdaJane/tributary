/**
 * Peaks → per-pixel drawing columns for the visible viewport. Pure: the
 * canvas component just strokes what this returns.
 */

/**
 * For each of `widthPx` columns starting at `startFrame`, the min/max
 * (normalized −1..1) over every peak bin the column covers. Columns beyond
 * the recorded bins are 0/0 (flat line).
 */
export function waveformColumns(
  pairs: Int16Array,
  samplesPerBin: number,
  fpp: number,
  startFrame: number,
  widthPx: number,
): Float32Array {
  const columns = new Float32Array(widthPx * 2);
  const binCount = pairs.length / 2;
  for (let x = 0; x < widthPx; x += 1) {
    const frameA = startFrame + x * fpp;
    const frameB = frameA + fpp;
    const firstBin = Math.floor(frameA / samplesPerBin);
    // The column owns bins starting before its right edge.
    const lastBin = Math.max(firstBin, Math.ceil(frameB / samplesPerBin) - 1);
    let min = 0;
    let max = 0;
    let seen = false;
    for (let bin = firstBin; bin <= lastBin && bin < binCount; bin += 1) {
      if (bin < 0) continue;
      const lo = pairs[bin * 2];
      const hi = pairs[bin * 2 + 1];
      if (!seen) {
        min = lo;
        max = hi;
        seen = true;
      } else {
        if (lo < min) min = lo;
        if (hi > max) max = hi;
      }
    }
    columns[x * 2] = min / 32_767;
    columns[x * 2 + 1] = max / 32_767;
  }
  return columns;
}
