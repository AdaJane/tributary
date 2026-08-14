/**
 * Pure merge of server events into the mixer mirror. The store applies;
 * this decides — separately testable, jsdom-free.
 */
import type { MixerState, StateDelta } from '../ws/messages';

/** A delta for a target this mirror doesn't know is stale news racing a
 * snapshot — dropped, and the snapshot that follows wins. */
function sameJack(
  patch: { device?: string | null; channel: number },
  device: string | null,
  channel: number,
): boolean {
  return (patch.device ?? null) === device && patch.channel === channel;
}

export function mergeDelta(state: MixerState, delta: StateDelta): MixerState {
  const strip = (id: number, patch: (s: MixerState['strips'][number]) => void): MixerState => {
    const strips = state.strips.map((s) => {
      if (s.id !== id) return s;
      const next = structuredClone(s);
      patch(next);
      return next;
    });
    return { ...state, strips };
  };

  switch (delta.kind) {
    case 'fader': {
      const level = delta.level_db;
      switch (delta.target.kind) {
        case 'strip':
          return strip(delta.target.id, (s) => {
            s.fader_db = level;
          });
        case 'bus': {
          const id = delta.target.id;
          return {
            ...state,
            buses: state.buses.map((b) =>
              b.id === id ? { ...b, fader_db: level } : b,
            ),
          };
        }
        case 'master':
          return { ...state, master: { ...state.master, fader_db: level } };
      }
      break;
    }
    case 'gain':
      return strip(delta.strip, (s) => {
        s.gain_db = delta.gain_db;
      });
    case 'eq_band':
      return strip(delta.strip, (s) => {
        const slot =
          delta.band === 'low_shelf'
            ? s.eq.low
            : delta.band === 'peak'
              ? s.eq.mid
              : s.eq.high;
        slot.freq_hz = delta.freq_hz;
        slot.gain_db = delta.gain_db;
        slot.q = delta.q;
      });
    case 'eq_enabled':
      return strip(delta.strip, (s) => {
        s.eq.enabled = delta.enabled;
      });
    case 'pan':
      return strip(delta.strip, (s) => {
        s.pan = delta.pan;
      });
    case 'mute':
      if (delta.target.kind === 'strip') {
        const mute = delta.mute;
        return strip(delta.target.id, (s) => {
          s.mute = mute;
        });
      }
      if (delta.target.kind === 'bus') {
        const id = delta.target.id;
        return {
          ...state,
          buses: state.buses.map((b) =>
            b.id === id ? { ...b, mute: delta.mute } : b,
          ),
        };
      }
      return state;
    case 'pfl':
      if (delta.target.kind === 'strip') {
        const on = delta.on;
        return strip(delta.target.id, (s) => {
          s.pfl = on;
        });
      }
      if (delta.target.kind === 'bus') {
        const id = delta.target.id;
        return {
          ...state,
          buses: state.buses.map((b) => (b.id === id ? { ...b, pfl: delta.on } : b)),
        };
      }
      return state;
    case 'input':
      return strip(delta.strip, (s) => {
        s.input = delta.input ?? null;
      });
    case 'renamed':
      if (delta.target.kind === 'strip') {
        const name = delta.name;
        return strip(delta.target.id, (s) => {
          s.name = name;
        });
      }
      if (delta.target.kind === 'bus') {
        const id = delta.target.id;
        return {
          ...state,
          buses: state.buses.map((b) =>
            b.id === id ? { ...b, name: delta.name } : b,
          ),
        };
      }
      return state;
    case 'send':
      return strip(delta.strip, (s) => {
        const existing = s.sends.find((send) => send.dest === delta.dest);
        if (existing) {
          existing.level_db = delta.level_db;
          existing.tap = delta.tap;
        } else {
          s.sends.push({ dest: delta.dest, level_db: delta.level_db, tap: delta.tap });
        }
      });
    case 'fx_params': {
      const fxId = delta.fx;
      return {
        ...state,
        fx: state.fx.map((f) => (f.id === fxId ? { ...f, params: delta.params } : f)),
      };
    }
    case 'fx_return': {
      const fxId = delta.fx;
      return {
        ...state,
        fx: state.fx.map((f) =>
          f.id === fxId ? { ...f, return_level_db: delta.level_db } : f,
        ),
      };
    }
    case 'route':
      return strip(delta.strip, (s) => {
        s.route_to = delta.to;
      });
    // Structural deltas ride REST replies; the broadcast is a full
    // snapshot, so merging them here is belt-and-braces.
    case 'strip_added':
      return state.strips.some((s) => s.id === delta.strip.id)
        ? state
        : { ...state, strips: [...state.strips, delta.strip] };
    case 'strip_removed':
      return { ...state, strips: state.strips.filter((s) => s.id !== delta.id) };
    case 'bus_added':
      return state.buses.some((b) => b.id === delta.bus.id)
        ? state
        : { ...state, buses: [...state.buses, delta.bus] };
    case 'bus_removed':
      return { ...state, buses: state.buses.filter((b) => b.id !== delta.id) };
    case 'record_arm':
      if (delta.target.kind === 'strip') {
        const armed = delta.armed;
        return strip(delta.target.id, (s) => {
          s.record_arm = armed;
        });
      }
      if (delta.target.kind === 'master') {
        return { ...state, master: { ...state.master, record_arm: delta.armed } };
      }
      return state;
    case 'record_arm_all':
      return {
        ...state,
        strips: state.strips.map((s) => ({ ...s, record_arm: delta.armed })),
      };
    // Keyed by the JACK on all three: one jack holds one feed, so a patch
    // replaces rather than joins.
    case 'output_patched': {
      const others = (state.outputs ?? []).filter(
        (o) => !sameJack(o, delta.patch.device ?? null, delta.patch.channel),
      );
      return { ...state, outputs: [...others, delta.patch] };
    }
    case 'output_unpatched':
      return {
        ...state,
        outputs: (state.outputs ?? []).filter(
          (o) => !sameJack(o, delta.jack.device ?? null, delta.jack.channel),
        ),
      };
    case 'output_tap':
      return {
        ...state,
        outputs: (state.outputs ?? []).map((o) =>
          sameJack(o, delta.jack.device ?? null, delta.jack.channel)
            ? { ...o, tap: delta.tap }
            : o,
        ),
      };
  }
  return state;
}
