/** Reconnect pacing: exponential with jitter, so a daemon restart doesn't
 * get a thundering herd of synchronized tabs. */
export const BACKOFF_BASE_MS = 1_000;
export const BACKOFF_MAX_MS = 30_000;

/** `random` is injected for testability (0..1). */
export function backoffDelay(attempt: number, random: () => number): number {
  const exponential = Math.min(
    BACKOFF_MAX_MS,
    BACKOFF_BASE_MS * 2 ** Math.min(attempt, 10),
  );
  // 50%–100% of the window: jitter without ever being instant.
  return exponential * (0.5 + random() * 0.5);
}
