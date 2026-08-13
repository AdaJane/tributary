/**
 * Pure presentation rules for the take browser. No DTO imports — the store
 * maps wire shapes; this file only decides how a take reads.
 */

/** "T03" — zero-padded so a list of takes stays column-aligned. */
export function takeLabel(take: number): string {
  return `T${String(take).padStart(2, '0')}`;
}

/**
 * Wall-clock time of day the take started. The formatter is injected so a
 * test can pin a locale and zone — otherwise this asserts differently on
 * every machine.
 */
export function takeTime(startedAtUnix: number, format: Intl.DateTimeFormat): string {
  return format.format(new Date(startedAtUnix * 1000));
}

/** "3:07", or "1:02:11" once it runs past an hour. */
export function takeDuration(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  const s = whole % 60;
  const m = Math.floor(whole / 60) % 60;
  const h = Math.floor(whole / 3600);
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function trackCountLabel(count: number): string {
  return count === 1 ? '1 track' : `${count} tracks`;
}

/**
 * Whether PLAY will actually roll this take, and why not.
 *
 * There is no resampler, so a take cut at another rate can be selected and
 * looked at but not played. The console prints this rather than letting
 * the user discover it as a 422 — and it stops being an exotic case the
 * moment anyone uses the restart-gated sample-rate switch, because every
 * pre-restart take is then mismatched.
 */
export function takePlayability(
  takeRate: number,
  engineRate: number,
): { playable: boolean; reason: string | null } {
  if (takeRate === engineRate) return { playable: true, reason: null };
  return {
    playable: false,
    reason: `cut at ${kHz(takeRate)} · the engine runs ${kHz(engineRate)}`,
  };
}

/** "44.1 kHz", "48 kHz" — no trailing zeros. */
export function kHz(rate: number): string {
  const k = rate / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(1)} kHz`;
}

/** Newest first, and never trusting the wire to have sorted. */
export function sortedTakes<T extends { take: number }>(takes: readonly T[]): T[] {
  return [...takes].sort((a, b) => b.take - a.take);
}
