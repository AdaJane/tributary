import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';

import { EMPTY_MIXER, useMixer } from '../../state/mixer';
import { useUi } from '../../state/ui';
import type { MixerState, StripState } from '../../ws/messages';
import { EqSection } from './EqSection';

/** A strip with a worked EQ: cut lows, boosted swept mid. */
function strip(): StripState {
  return {
    id: 0,
    name: 'Vocal',
    input: null,
    gain_db: 0,
    eq: {
      enabled: true,
      low: { kind: 'low_shelf', freq_hz: 120, gain_db: -6, q: 0.9 },
      mid: { kind: 'peak', freq_hz: 2500, gain_db: 4, q: 1.2 },
      high: { kind: 'high_shelf', freq_hz: 12000, gain_db: 0, q: 0.71 },
    },
    sends: [],
    pan: 0,
    fader_db: 0,
    mute: false,
    pfl: false,
    record_arm: false,
    route_to: { kind: 'master' },
  };
}

function seeded(): MixerState {
  return { ...EMPTY_MIXER, strips: [strip()] };
}

describe('EqSection', () => {
  beforeEach(() => {
    useMixer.getState().applySnapshot(seeded());
    useUi.getState().setEqExpanded('0', true);
  });

  it('the On toggle flips engagement optimistically in the mirror', async () => {
    const user = userEvent.setup();
    render(<EqSection strip={strip()} />);
    const toggle = screen.getByRole('button', { name: 'EQ engaged for Vocal' });
    expect(toggle).toHaveAttribute('aria-pressed', 'true');
    await user.click(toggle);
    expect(useMixer.getState().state.strips[0].eq.enabled).toBe(false);
  });

  it('Reset flattens every band but leaves engagement alone', async () => {
    const user = userEvent.setup();
    render(<EqSection strip={strip()} />);
    await user.click(screen.getByRole('button', { name: 'Reset Vocal EQ to flat' }));
    const eq = useMixer.getState().state.strips[0].eq;
    expect(eq.low).toEqual({ kind: 'low_shelf', freq_hz: 80, gain_db: 0, q: 0.71 });
    expect(eq.mid).toEqual({ kind: 'peak', freq_hz: 800, gain_db: 0, q: 0.71 });
    expect(eq.high).toEqual({ kind: 'high_shelf', freq_hz: 12000, gain_db: 0, q: 0.71 });
    expect(eq.enabled).toBe(true);
  });
});
