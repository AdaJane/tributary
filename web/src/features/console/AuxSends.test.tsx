import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { EMPTY_MIXER, useMixer } from '../../state/mixer';
import type { MixerState, StripState } from '../../ws/messages';
import { AuxSends } from './AuxSends';

function strip(): StripState {
  return {
    id: 0,
    name: 'Vocal',
    input: null,
    gain_db: 0,
    eq: {
      enabled: true,
      low: { kind: 'low_shelf', freq_hz: 80, gain_db: 0, q: 0.71 },
      mid: { kind: 'peak', freq_hz: 800, gain_db: 0, q: 0.71 },
      high: { kind: 'high_shelf', freq_hz: 12000, gain_db: 0, q: 0.71 },
    },
    sends: [{ dest: 0, level_db: -12, tap: 'post_fader' }],
    pan: 0,
    fader_db: 0,
    mute: false,
    pfl: false,
    record_arm: false,
    route_to: { kind: 'master' },
  };
}

function console_state(): MixerState {
  return {
    ...EMPTY_MIXER,
    strips: [strip()],
    buses: [
      { id: 0, kind: 'aux', name: 'FX 1', fader_db: 0, pan: 0, mute: false, pfl: false },
      { id: 1, kind: 'aux', name: 'FX 2', fader_db: 0, pan: 0, mute: false, pfl: false },
      { id: 2, kind: 'group', name: 'Band', fader_db: 0, pan: 0, mute: false, pfl: false },
    ],
  };
}

describe('AuxSends', () => {
  beforeEach(() => {
    useMixer.getState().applySnapshot(console_state());
  });

  /** Regression: a store selector that filtered inline returned a fresh
   * array every call, which React read as an ever-changing snapshot — an
   * infinite re-render loop that blanked the whole console. Rendering at
   * all (React throws on update-depth overflow) is the assertion. */
  it('renders one knob per aux bus without looping', () => {
    render(<AuxSends strip={strip()} />);
    expect(screen.getByRole('slider', { name: 'Aux 1' })).toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Aux 2' })).toBeInTheDocument();
    expect(screen.queryByRole('slider', { name: 'Aux 3' })).not.toBeInTheDocument();
  });

  it('shows the stored send level and floors untouched sends', () => {
    render(<AuxSends strip={strip()} />);
    expect(screen.getByRole('slider', { name: 'Aux 1' })).toHaveAttribute(
      'aria-valuenow',
      '-12',
    );
    expect(screen.getByRole('slider', { name: 'Aux 2' })).toHaveAttribute(
      'aria-valuetext',
      '-∞',
    );
  });
});
