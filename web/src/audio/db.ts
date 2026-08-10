/**
 * dB semantics shared across the console. The wire constants must match
 * trib-core's `db.rs` exactly — the daemon refuses values outside them.
 */
export const FADER_MIN_DB = -90;
export const FADER_MAX_DB = 10;
export const GAIN_MIN_DB = -20;
export const GAIN_MAX_DB = 60;

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** "+2.5", "0.0", "-6.0", and the floor renders as "-∞". */
export function formatDb(db: number): string {
  if (db <= FADER_MIN_DB) return '-∞';
  const fixed = db.toFixed(1);
  return db > 0 ? `+${fixed}` : fixed;
}
