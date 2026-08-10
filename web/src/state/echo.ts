/**
 * The anti-fight rule for optimistic controls. A param is HOT while its
 * control is captured and for HOT_LINGER_MS after the last local send;
 * while hot, foreign echoes for that param are dropped (local is
 * authoritative under the finger). Our own acks always release the param —
 * the server is authoritative at rest.
 */
export const HOT_LINGER_MS = 300;

export interface HotState {
  /** paramKey → hot-until timestamp (Infinity while captured). */
  readonly hot: ReadonlyMap<string, number>;
}

export function markCaptured(state: HotState, paramKey: string): HotState {
  const hot = new Map(state.hot);
  hot.set(paramKey, Infinity);
  return { hot };
}

/** Capture released or a send fired: hot lingers briefly past the gesture. */
export function markSent(state: HotState, paramKey: string, now: number): HotState {
  const hot = new Map(state.hot);
  hot.set(paramKey, now + HOT_LINGER_MS);
  return { hot };
}

export function releaseHot(state: HotState, paramKey: string, now: number): HotState {
  const hot = new Map(state.hot);
  hot.set(paramKey, now + HOT_LINGER_MS);
  return { hot };
}

/**
 * Should an incoming state_changed echo be applied to the mirror?
 * - Our own echo (ack.client_id matches): always — it confirms a send and
 *   carries any server-side clamping.
 * - Foreign echo while the param is hot: dropped (we're mid-gesture).
 * - Anything else: applied.
 */
export function shouldAcceptEcho(
  state: HotState,
  paramKey: string,
  ackClientId: string | undefined,
  ownClientId: string,
  now: number,
): boolean {
  if (ackClientId === ownClientId) return true;
  const hotUntil = state.hot.get(paramKey) ?? 0;
  return hotUntil <= now;
}

export const EMPTY_HOT: HotState = { hot: new Map() };
