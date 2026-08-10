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

  constructor(intervalMs: number, flush: (batch: Map<string, T>) => void) {
    this.intervalMs = intervalMs;
    this.flush = flush;
  }

  push(key: string, value: T): void {
    this.pending.set(key, value);
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
