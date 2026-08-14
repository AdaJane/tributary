import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useMixer } from '../../state/mixer';
import { useOutputs } from '../../state/outputs';
import type { MixerState } from '../../ws/messages';
import { OutputPatchbayModal } from './OutputPatchbayModal';

vi.mock('../../api/client', () => ({
  $api: {
    GET: vi.fn().mockResolvedValue({ data: undefined }),
    PUT: vi.fn().mockResolvedValue({ data: undefined, response: { status: 200 } }),
    POST: vi.fn().mockResolvedValue({ data: undefined }),
  },
}));

function device(over: Record<string, unknown> = {}) {
  return {
    name: 'interface',
    label: null,
    channels: 4,
    monitor_channels: 0,
    active: false,
    status: 'open',
    patched: true,
    underruns: 0,
    overruns: 0,
    xruns: 0,
    worst_block_us: 0,
    muted: false,
    volume_percent: null,
    error: null,
    ...over,
  };
}

function mixer(): MixerState {
  return {
    strips: [
      {
        id: 0,
        name: 'Kick',
        input: null,
        gain_db: 0,
        eq: {
          enabled: false,
          low: { kind: 'low_shelf', freq_hz: 80, gain_db: 0, q: 0.71 },
          mid: { kind: 'peak', freq_hz: 800, gain_db: 0, q: 0.71 },
          high: { kind: 'high_shelf', freq_hz: 12000, gain_db: 0, q: 0.71 },
        },
        sends: [],
        pan: 0,
        fader_db: 0,
        mute: false,
        pfl: false,
        record_arm: false,
        route_to: { kind: 'master' },
      },
    ],
    buses: [],
    fx: [],
    instruments: [],
    outputs: [],
    master: { fader_db: 0, record_arm: false },
  } as unknown as MixerState;
}

function setDocument(over: Record<string, unknown> = {}) {
  useOutputs.setState({
    document: {
      patches: [],
      reports: [],
      devices: [device()],
      supported: true,
      ...over,
    },
    loaded: true,
    pending: false,
  } as never);
}

describe('OutputPatchbayModal', () => {
  beforeEach(() => {
    useMixer.setState({ state: mixer(), loaded: true } as never);
    setDocument();
  });

  it('offers the master and the buses as sources, which no strip-only door could reach', () => {
    // There are no bus strips in this console and the master has no channel
    // strip. Organising the room by OUTPUT is what makes them reachable at
    // all — a room organised by source could only ever list strips.
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(screen.getByRole('option', { name: /OUT 1 on interface, free/ })).toBeTruthy();
  });

  it('an unplugged output a patch still names is drawn, not dropped', () => {
    // Without the ghost, the strip's OUT button prints a jack number with
    // nothing anywhere in the room to explain why it is silent.
    setDocument({
      devices: [],
      patches: [
        {
          device: 'unplugged',
          channel: 2,
          source_channel: 0,
          tap: 'pre_fader',
          source: { kind: 'master' },
        },
      ],
    });
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(screen.getByText('unplugged')).toBeTruthy();
    expect(screen.getByText(/not connected — patch these channels somewhere else/)).toBeTruthy();
  });

  it('a muted output says so in amber without calling itself failed', () => {
    // An output has no meter, so this line is the only place a dead PA is
    // explainable anywhere in the console.
    setDocument({ devices: [device({ muted: true })] });
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(screen.getByText(/Silent at the output: muted in the system mixer/)).toBeTruthy();
    expect(screen.queryByText('failed')).toBeNull();
  });

  it('a patch past the device keeps its socket and says there is no jack', () => {
    setDocument({
      devices: [device({ channels: 2 })],
      patches: [
        {
          device: 'interface',
          channel: 5,
          source_channel: 0,
          tap: 'pre_fader',
          source: { kind: 'master' },
        },
      ],
    });
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(
      screen.getByRole('option', { name: /OUT 6 on interface.*no jack on this device/ }),
    ).toBeTruthy();
  });

  it('a backend with no patchable outputs says so rather than drawing a dead room', () => {
    setDocument({ devices: [], supported: false });
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(screen.getByText(/no patchable outputs/)).toBeTruthy();
  });

  it('an occupied output prints who holds it before anything is clicked', () => {
    setDocument({
      patches: [
        {
          device: 'interface',
          channel: 1,
          source_channel: 0,
          tap: 'pre_fader',
          source: { kind: 'strip', id: 0 },
        },
      ],
    });
    render(<OutputPatchbayModal open onClose={() => {}} />);
    expect(screen.getByRole('option', { name: /OUT 2 on interface, fed by Kick/ })).toBeTruthy();
  });

  it('selecting an output reveals its tap switch and what the tap will do', async () => {
    const user = userEvent.setup();
    render(<OutputPatchbayModal open onClose={() => {}} />);
    await user.click(screen.getByRole('option', { name: /OUT 1 on interface/ }));
    expect(screen.getByText(/the fader and mute do not affect this output/)).toBeTruthy();
  });
});
