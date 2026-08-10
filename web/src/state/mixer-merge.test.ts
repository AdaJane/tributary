import { describe, expect, it } from 'vitest';

import type { MixerState, StripState } from '../ws/messages';
import { mergeDelta } from './mixer-merge';

function strip(id: number): StripState {
  return {
    id,
    name: `Ch ${id + 1}`,
    input: null,
    gain_db: 0,
    eq: {
      enabled: true,
      low: { kind: 'low_shelf', freq_hz: 80, gain_db: 0, q: 0.71 },
      mid: { kind: 'peak', freq_hz: 800, gain_db: 0, q: 0.71 },
      high: { kind: 'high_shelf', freq_hz: 12000, gain_db: 0, q: 0.71 },
    },
    sends: [],
    pan: 0,
    fader_db: -90,
    mute: false,
    pfl: false,
    record_arm: false,
    route_to: { kind: 'master' },
  };
}

function state(): MixerState {
  return {
    strips: [strip(0), strip(1)],
    buses: [],
    fx: [],
    master: { fader_db: 0, record_arm: false },
  };
}

describe('mergeDelta', () => {
  it('patches only the addressed strip and never mutates the input', () => {
    const before = state();
    const after = mergeDelta(before, {
      kind: 'fader',
      target: { kind: 'strip', id: 1 },
      level_db: -6,
    });
    expect(after.strips[1].fader_db).toBe(-6);
    expect(after.strips[0].fader_db).toBe(-90);
    expect(before.strips[1].fader_db).toBe(-90);
  });

  it('eq band deltas land in the right slot', () => {
    const after = mergeDelta(state(), {
      kind: 'eq_band',
      strip: 0,
      band: 'peak',
      freq_hz: 1200,
      gain_db: 4,
      q: 0.9,
    });
    expect(after.strips[0].eq.mid.freq_hz).toBe(1200);
    expect(after.strips[0].eq.low.gain_db).toBe(0);
  });

  it('a delta for an unknown strip is stale news, not a crash', () => {
    const before = state();
    const after = mergeDelta(before, {
      kind: 'gain',
      strip: 99,
      gain_db: 10,
    });
    expect(after.strips).toEqual(before.strips);
  });

  it('master fader and renames merge', () => {
    let s = mergeDelta(state(), {
      kind: 'fader',
      target: { kind: 'master' },
      level_db: -3,
    });
    s = mergeDelta(s, {
      kind: 'renamed',
      target: { kind: 'strip', id: 0 },
      name: 'Kick',
    });
    expect(s.master.fader_db).toBe(-3);
    expect(s.strips[0].name).toBe('Kick');
  });

  it('strip_added is idempotent against the snapshot race', () => {
    const before = state();
    const added = mergeDelta(before, { kind: 'strip_added', strip: strip(2) });
    expect(added.strips).toHaveLength(3);
    const again = mergeDelta(added, { kind: 'strip_added', strip: strip(2) });
    expect(again.strips).toHaveLength(3);
  });
});
