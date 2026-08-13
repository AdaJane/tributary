/**
 * Keyed frame batcher: WS bursts coalesce per key and flush on an interval,
 * so a burst costs one render rather than hundreds. Meters flush at 50 ms
 * (LED-live); document-shaped data can use a slower instance.
 */
export class FrameBatcher<T> {
  private pending = new Map<string, T>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private readonly intervalMs: number;
  private readonly flush: (batch: Map<string, T>) => void;
  private readonly merge: ((prev: T, next: T) => T) | null;

  /** `merge` decides what survives when one key is pushed twice inside a
   * window. Omit it for document-shaped data, where the newest value IS
   * the answer. Supply it for sampled data, where dropping the collision
   * loses a measurement — meters coalesce by max, exactly as the daemon's
   * own pump does before it publishes. */
  constructor(
    intervalMs: number,
    flush: (batch: Map<string, T>) => void,
    merge?: (prev: T, next: T) => T,
  ) {
    this.intervalMs = intervalMs;
    this.flush = flush;
    this.merge = merge ?? null;
  }

  push(key: string, value: T): void {
    const prev = this.pending.get(key);
    this.pending.set(
      key,
      prev !== undefined && this.merge ? this.merge(prev, value) : value,
    );
    this.timer ??= setInterval(() => this.tick(), this.intervalMs);
  }

  private tick(): void {
    if (this.pending.size === 0) {
      // Idle: stop ticking until the next push.
      if (this.timer !== null) {
        clearInterval(this.timer);
        this.timer = null;
      }
      return;
    }
    const batch = this.pending;
    this.pending = new Map();
    this.flush(batch);
  }

  stop(): void {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
    this.pending.clear();
  }
}
