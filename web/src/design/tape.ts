/**
 * Masking-tape geometry. Rotation is a deterministic hash of the id so tape
 * never re-shuffles across renders or sessions.
 */
export const TAPE_ROTATE_MAX_DEG = 1.2;

/** FNV-1a over the id, folded into [-max, +max]. */
export function rotationFor(id: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    hash ^= id.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  const unit = (hash >>> 0) / 0xffffffff;
  return (unit * 2 - 1) * TAPE_ROTATE_MAX_DEG;
}
