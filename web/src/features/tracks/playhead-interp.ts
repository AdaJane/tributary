import type { LoopRegion } from '../../state/transport';

/**
 * Estimated playhead now: the last daemon-reported frame plus real time
 * elapsed since, wrapped into the loop and clamped to the take. Pure so
 * the rAF loop stays dumb.
 */
export function estimatePosition(
  lastFrames: number,
  lastAtMs: number,
  nowMs: number,
  sampleRate: number,
  playing: boolean,
  totalFrames: number,
  loop: LoopRegion | null,
): number {
  if (!playing) return Math.min(lastFrames, totalFrames);
  const elapsed = Math.max(0, nowMs - lastAtMs) / 1000;
  let estimate = lastFrames + elapsed * sampleRate;
  if (loop && estimate >= loop.endFrames && loop.endFrames > loop.startFrames) {
    const span = loop.endFrames - loop.startFrames;
    estimate = loop.startFrames + ((estimate - loop.startFrames) % span);
  }
  return Math.min(estimate, totalFrames);
}
