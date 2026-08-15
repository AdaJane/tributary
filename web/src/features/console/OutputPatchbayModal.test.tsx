import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useMidi } from '../../state/midi';
import { useMixer } from '../../state/mixer';
import { useOutputs } from '../../state/outputs';
import { useTransport } from '../../state/transport';
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

function setMidi(over: Record<string, unknown> = {}) {
  useMidi.setState({
    document: {
      routes: [],
      reports: [],
      inputs: [],
      outputs: [],
      take_tracks: [],
      ...over,
    },
    loaded: true,
    pending: false,
  } as never);
}

describe('OutputPatchbayModal', () => {
  beforeEach(() => {
    useMixer.setState({ state: mixer(), loaded: true } as never);
    useTransport.setState({ phase: 'stopped' } as never);
    setDocument();
    setMidi();
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

  it('refuses to re-patch while the tape rolls, and says why beside the control', async () => {
    // A control that vanishes teaches nothing, and the daemon's 409 arrives
    // only after the gesture — by which point the user has already been
    // told "no" without being told anything.
    useTransport.setState({ phase: 'recording' } as never);
    const user = userEvent.setup();
    render(<OutputPatchbayModal open onClose={() => {}} />);
    await user.click(screen.getByRole('option', { name: /OUT 1 on interface/ }));
    expect(screen.getByText(/would cut every reverb tail onto the tape/)).toBeTruthy();
    expect(screen.getByLabelText('Source')).toHaveProperty('disabled', true);
  });

  describe('MIDI rows', () => {
    const juno = {
      id: 'Juno',
      name: 'Juno',
      connected: true,
      absent: false,
      routes: 1,
      sent: 0,
      errors: 0,
    };
    const echo = { kind: 'instrument', id: 0 };

    function withRack() {
      useMixer.setState({
        state: { ...mixer(), instruments: [{ id: 0, name: 'Rhodes' }] },
        loaded: true,
      } as never);
    }

    it('draws a MIDI port under its own chip, never as an audio jack', () => {
      // The two halves of the bay obey different rules — one feed against a
      // merge — so they must not be confusable at a glance.
      setMidi({ outputs: [juno] });
      render(<OutputPatchbayModal open onClose={() => {}} />);
      expect(screen.getByText('MIDI')).toBeTruthy();
      expect(screen.getByRole('option', { name: 'Add a route to Juno' })).toBeTruthy();
    });

    it('offers MIDI sources only — a strip is never on the list', async () => {
      // A direct out carries audio and a MIDI port carries events. Offering
      // Kick here would be offering a conversion the box cannot do.
      withRack();
      setMidi({ outputs: [juno], inputs: [], take_tracks: [] });
      const user = userEvent.setup();
      render(<OutputPatchbayModal open onClose={() => {}} />);
      await user.click(screen.getByRole('option', { name: 'Add a route to Juno' }));
      const source = screen.getByLabelText('Source') as HTMLSelectElement;
      const labels = [...source.options].map((o) => o.textContent);
      expect(labels).toContain('Rhodes (echo)');
      expect(labels).not.toContain('Kick');
    });

    it('shows the tap switch disabled with its reason, rather than hiding it', async () => {
      withRack();
      setMidi({ outputs: [juno] });
      const user = userEvent.setup();
      render(<OutputPatchbayModal open onClose={() => {}} />);
      await user.click(screen.getByRole('option', { name: 'Add a route to Juno' }));
      expect(screen.getByText(/MIDI carries notes, not a signal to tap/)).toBeTruthy();
    });

    it('puts two routes on one port, because MIDI merges where audio would sum', () => {
      withRack();
      setMidi({
        outputs: [{ ...juno, routes: 2 }],
        inputs: [{ id: 'nanoKEY2', name: 'nanoKEY2', connected: true, absent: false }],
        routes: [
          { port: 'Juno', channel: null, source: echo },
          { port: 'Juno', channel: 9, source: { kind: 'port', name: 'nanoKEY2' } },
        ],
        reports: [
          { port: 'Juno', status: 'live', reason: null },
          { port: 'Juno', status: 'live', reason: null },
        ],
      });
      render(<OutputPatchbayModal open onClose={() => {}} />);
      expect(screen.getByRole('option', { name: /Rhodes \(echo\) to Juno, live/ })).toBeTruthy();
      expect(screen.getByRole('option', { name: /nanoKEY2 \(thru\) to Juno, live/ })).toBeTruthy();
      expect(screen.getByText('ch 10')).toBeTruthy();
    });

    it("blames the keyboard, not the port, when only the keyboard is gone", () => {
      // Both ends can be missing and they are different problems. Sending
      // somebody to check the wrong cable is the failure this prevents.
      withRack();
      setMidi({
        outputs: [juno],
        routes: [{ port: 'Juno', channel: null, source: { kind: 'port', name: 'nanoKEY2' } }],
        reports: [
          { port: 'Juno', status: 'ready', reason: 'its input “nanoKEY2” is not connected' },
        ],
      });
      render(<OutputPatchbayModal open onClose={() => {}} />);
      expect(screen.getByText(/its input “nanoKEY2” is not connected/)).toBeTruthy();
      expect(screen.queryByText(/this MIDI port is not connected/)).toBeNull();
    });

    it('says nothing has been sent, because a MIDI port has no meter', () => {
      withRack();
      setMidi({
        outputs: [juno],
        routes: [{ port: 'Juno', channel: null, source: echo }],
        reports: [{ port: 'Juno', status: 'live', reason: null }],
      });
      render(<OutputPatchbayModal open onClose={() => {}} />);
      expect(screen.getByText('nothing sent yet')).toBeTruthy();
    });

    it('stays editable while the tape rolls, unlike a direct out', async () => {
      // The whole reason MIDI routes are ParamOnly: no graph swap, so no
      // reverb tail to cut, so no reason to refuse the gesture.
      withRack();
      useTransport.setState({ phase: 'recording' } as never);
      setMidi({ outputs: [juno] });
      const user = userEvent.setup();
      render(<OutputPatchbayModal open onClose={() => {}} />);
      await user.click(screen.getByRole('option', { name: 'Add a route to Juno' }));
      expect(screen.getByLabelText('Source')).toHaveProperty('disabled', false);
    });
  });
});
