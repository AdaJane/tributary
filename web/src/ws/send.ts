/**
 * The control write path: optimistic local apply + throttled WS `set` with
 * per-tab identity, and the hot-window bookkeeping the echo rule reads.
 */
import { useMixer } from '../state/mixer';
import {
  EMPTY_HOT,
  type HotState,
  markCaptured,
  markSent,
  shouldAcceptEcho,
} from '../state/echo';
import { GestureThrottle, SEND_WINDOW_MS } from '../state/throttle';
import { wsClient } from './client-instance';
import type { MixCommand, StateDelta, WsAck } from './messages';
import { uuidv4 } from './uuid';

export const CLIENT_ID: string = uuidv4();

let seq = 0;
let hot: HotState = EMPTY_HOT;

const throttle = new GestureThrottle<MixCommand>(SEND_WINDOW_MS, (key, command) => {
  seq += 1;
  hot = markSent(hot, key, Date.now());
  wsClient.send({ op: 'set', command, seq, client_id: CLIENT_ID });
});

/** Identity of the parameter a delta/command addresses — the hot-window key. */
export function paramKey(delta: StateDelta): string {
  switch (delta.kind) {
    case 'fader':
      return `fader:${JSON.stringify(delta.target)}`;
    case 'gain':
      return `gain:${delta.strip}`;
    case 'eq_band':
      return `eq:${delta.strip}:${delta.band}`;
    case 'eq_enabled':
      return `eqon:${delta.strip}`;
    case 'pan':
      return `pan:${delta.strip}`;
    case 'mute':
      return `mute:${JSON.stringify(delta.target)}`;
    case 'pfl':
      return `pfl:${JSON.stringify(delta.target)}`;
    case 'input':
      return `input:${delta.strip}`;
    case 'renamed':
      return `name:${JSON.stringify(delta.target)}`;
    case 'strip_added':
      return `strip+:${delta.strip.id}`;
    case 'strip_removed':
      return `strip-:${delta.id}`;
    case 'send':
      return `send:${delta.strip}:${delta.dest}`;
    case 'route':
      return `route:${delta.strip}`;
    case 'fx_params':
      return `fxp:${delta.fx}`;
    case 'fx_return':
      return `fxr:${delta.fx}`;
    case 'bus_added':
      return `bus+:${delta.bus.id}`;
    case 'bus_removed':
      return `bus-:${delta.id}`;
    case 'record_arm':
      return `arm:${JSON.stringify(delta.target)}`;
    case 'record_arm_all':
      return 'arm:all';
    // Instrument edits are REST, not gestures — no drag produces them, so
    // they never enter the hot window. They still need a key here because
    // the switch is exhaustive by design: a new delta kind must be a
    // compile error until somebody decides what it means.
    case 'instrument_added':
      return `inst+:${delta.instrument.id}`;
    case 'instrument_removed':
      return `inst-:${delta.id}`;
    case 'instrument_changed':
      return `inst:${delta.instrument.id}`;
    case 'instrument_strips_added':
      return `inst-strips:${delta.id}`;
    // Output patches are REST too, for the same reason: the room is a
    // two-click picker, not a drag. Keyed by the JACK, because the jack is
    // the identity — one jack holds one feed.
    case 'output_patched':
      return `out:${delta.patch.device ?? ''}#${delta.patch.channel}`;
    case 'output_unpatched':
    case 'output_tap':
      return `out:${delta.jack.device ?? ''}#${delta.jack.channel}`;
  }
}

/** A continuous gesture: apply locally now, send throttled. `delta` is the
 * optimistic mirror patch; `command` the server mutation. */
export function gesture(delta: StateDelta, command: MixCommand): void {
  const key = paramKey(delta);
  hot = markCaptured(hot, key);
  useMixer.getState().setLocal(delta);
  throttle.push(key, command);
}

export function gestureEnd(delta: StateDelta): void {
  hot = markSent(hot, paramKey(delta), Date.now());
}

/** The echo rule, bound to this tab's identity and hot table. */
export function acceptEcho(delta: StateDelta, ack: WsAck | null | undefined): boolean {
  return shouldAcceptEcho(
    hot,
    paramKey(delta),
    ack?.client_id,
    CLIENT_ID,
    Date.now(),
  );
}
