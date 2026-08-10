/**
 * Trailing-edge throttle for continuous gestures: at most one send per
 * window per key, and the FINAL value always goes out — a fader must land
 * exactly where the finger left it.
 */
export const SEND_WINDOW_MS = 33; // ~30 msg/s per param

export class GestureThrottle<T> {
  private lastSent = new Map<string, number>();
  private trailing = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly windowMs: number;
  private readonly send: (key: string, value: T) => void;

  constructor(windowMs: number, send: (key: string, value: T) => void) {
    this.windowMs = windowMs;
    this.send = send;
  }

  push(key: string, value: T, now = Date.now()): void {
    const since = now - (this.lastSent.get(key) ?? 0);
    const pending = this.trailing.get(key);
    if (pending !== undefined) clearTimeout(pending);
    if (since >= this.windowMs) {
      this.lastSent.set(key, now);
      this.trailing.delete(key);
      this.send(key, value);
    } else {
      this.trailing.set(
        key,
        setTimeout(() => {
          this.trailing.delete(key);
          this.lastSent.set(key, Date.now());
          this.send(key, value);
        }, this.windowMs - since),
      );
    }
  }
}
