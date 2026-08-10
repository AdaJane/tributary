/**
 * The long-throw fader taper: piecewise-linear in dB over cap position
 * t ∈ [0,1], with unity at 75% travel like the reference console. Both
 * directions are exact inverses so drag → send → echo never drifts.
 */
import { FADER_MIN_DB, clamp } from './db';

/** (position, dB) anchors, ascending. */
const ANCHORS: ReadonlyArray<readonly [number, number]> = [
  [0, FADER_MIN_DB],
  [0.05, -60],
  [0.25, -30],
  [0.5, -10],
  [0.75, 0],
  [1, 10],
];

export function positionToDb(position: number): number {
  const t = clamp(position, 0, 1);
  for (let i = 1; i < ANCHORS.length; i++) {
    const [t1, db1] = ANCHORS[i];
    if (t <= t1) {
      const [t0, db0] = ANCHORS[i - 1];
      return db0 + ((t - t0) / (t1 - t0)) * (db1 - db0);
    }
  }
  return ANCHORS[ANCHORS.length - 1][1];
}

export function dbToPosition(db: number): number {
  const d = clamp(db, FADER_MIN_DB, ANCHORS[ANCHORS.length - 1][1]);
  for (let i = 1; i < ANCHORS.length; i++) {
    const [t1, db1] = ANCHORS[i];
    if (d <= db1) {
      const [t0, db0] = ANCHORS[i - 1];
      return t0 + ((d - db0) / (db1 - db0)) * (t1 - t0);
    }
  }
  return 1;
}
