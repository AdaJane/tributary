import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { $api } from '../../api/client';
import { useAudioStatus } from '../../state/audio';
import { useDevices } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import { InputPickerModal } from './InputPickerModal';

// The modal's open/Refresh hits POST /api/v1/devices/refresh; feed it a
// two-box system unless a test overrides the mock.
vi.mock('../../api/client', () => ({
  $api: {
    GET: vi.fn().mockResolvedValue({ data: undefined }),
    PUT: vi.fn().mockResolvedValue({ data: [] }),
    POST: vi.fn().mockResolvedValue({
      data: [
        {
          name: 'default',
          channels: 2,
          active: true,
          status: 'open',
          patched: true,
          underruns: 0,
          reconciled_from: null,
        },
        {
          name: 'dock',
          channels: 2,
          active: false,
          status: 'available',
          patched: false,
          underruns: 0,
          reconciled_from: null,
        },
      ],
    }),
  },
}));

function strip(id: number, name: string, device: string | null, channel: number | null): StripState {
  return {
    id,
    name,
    input:
      channel === null ? null : { device: device ?? undefined, device_channel: channel },
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

const kick = strip(0, 'Kick', null, 0);
const snare = strip(1, 'Snare', null, null);
const overhead = strip(2, 'OH', 'dock', 1);
const console_ = [kick, snare, overhead];

beforeEach(() => {
  useDevices.setState({ devices: [], loaded: false });
  useAudioStatus.setState({ status: null });
});

describe('when the audio backend has no card', () => {
  it('says so once, instead of showing an empty room', async () => {
    vi.mocked($api.GET).mockImplementationOnce((async (path: string) =>
      path === '/api/v1/audio'
        ? {
            data: {
              layer: 'exclusive',
              backend: 'alsa',
              started: true,
              running: false,
              card: null,
              error: 'hw:0 (Capture): No such file or directory',
              realtime: { scheduling: 'fifo', priority: 10, memory_locked: true, reason: null },
            },
          }
        : { data: undefined }) as never);
    vi.mocked($api.POST).mockResolvedValueOnce({ data: [] } as never);
    render(
      <InputPickerModal strip={kick} strips={console_} open onClose={() => {}} onPatch={() => {}} />,
    );
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      'Real-time card not open: hw:0 (Capture): No such file or directory — retrying',
    );
    expect(screen.queryByText(/No input device detected/)).toBeNull();
  });
});

describe('InputPickerModal', () => {
  it('draws one lettered section per device with honest jack states', async () => {
    render(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={() => {}}
      />,
    );
    // The refresh lands async; sections follow.
    expect(await screen.findByText('System default input')).toBeInTheDocument();
    expect(screen.getByText('dock')).toBeInTheDocument();
    expect(
      screen.getByRole('option', { name: 'IN 1 on System default input, in use by Kick' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('option', { name: 'B2 on dock, in use by OH' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('option', { name: 'IN 2 on System default input, free' }),
    ).toBeInTheDocument();
    // Two devices, two jacks each.
    expect(screen.getAllByRole('option')).toHaveLength(4);
  });

  it('a failed section explains why', () => {
    useDevices.setState({
      devices: [
        {
          name: 'alsa_input.usb-UMC1820.multichannel-input',
          label: 'UMC1820 Multichannel',
          channels: 10,
          active: true,
          status: 'failed',
          patched: true,
          underruns: 0,
          overruns: 0,
          profiles: [],
          muted: false,
          error: 'audio backend not running',
          reconciled_from: null,
        },
      ],
      loaded: true,
    });
    render(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={() => {}}
      />,
    );
    // The default box and the source's own box both explain the failure.
    const alerts = screen.getAllByRole('alert');
    expect(alerts).toHaveLength(2);
    expect(alerts[0]).toHaveTextContent('audio backend not running');
  });

  it('a muted device says so, without calling itself failed', () => {
    // The bug this exists for: a muted source reports open/patched with no
    // error, so the patchbay looked identical to a healthy one while every
    // meter behind it read silence.
    useDevices.setState({
      devices: [
        {
          name: 'alsa_input.usb-UMC1820.multichannel-input',
          label: 'UMC1820 Multichannel',
          channels: 10,
          active: true,
          status: 'open',
          patched: true,
          underruns: 0,
          overruns: 0,
          profiles: [],
          muted: true,
          error: null,
          reconciled_from: null,
        },
      ],
      loaded: true,
    });
    render(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={() => {}}
      />,
    );
    expect(screen.queryAllByRole('alert')).toHaveLength(0);
    // Both the default box and the source's own box carry the reason.
    expect(
      screen.getAllByText(/Silent at the source: muted in the system mixer/),
    ).toHaveLength(2);
  });

  it('patching hands back the device-qualified jack', async () => {
    const user = userEvent.setup();
    const onPatch = vi.fn();
    render(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={onPatch}
      />,
    );
    await user.click(await screen.findByRole('option', { name: 'B1 on dock, free' }));
    // The modal hands back a discriminated source, not a bare jack: an
    // instrument channel and a device channel are both patches, and the
    // caller must not have to guess which one it was handed.
    expect(onPatch).toHaveBeenCalledWith({ kind: 'device', device: 'dock', channel: 0 });
    await user.click(
      screen.getByRole('option', { name: 'IN 1 on System default input, in use by Kick' }),
    );
    expect(onPatch).toHaveBeenCalledWith({ kind: 'device', device: null, channel: 0 });
  });

  it('a dead patch past the device stays visible and marked', async () => {
    const wide = strip(3, 'Amb', 'dock', 5);
    render(
      <InputPickerModal
        strip={snare}
        strips={[...console_, wide]}
        open
        onClose={() => {}}
        onPatch={() => {}}
      />,
    );
    expect(
      await screen.findByRole('option', {
        name: 'B6 on dock, in use by Amb, no jack on this device',
      }),
    ).toBeInTheDocument();
  });

  it('disconnect patches null and is disabled when already unpatched', async () => {
    const user = userEvent.setup();
    const onPatch = vi.fn();
    const { rerender } = render(
      <InputPickerModal
        strip={kick}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={onPatch}
      />,
    );
    await user.click(
      await screen.findByRole('button', { name: 'Disconnect input from Kick' }),
    );
    expect(onPatch).toHaveBeenCalledWith(null);
    rerender(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={onPatch}
      />,
    );
    expect(
      screen.getByRole('button', { name: 'Disconnect input from Snare' }),
    ).toBeDisabled();
  });

  it('the refresh button re-enumerates on demand', async () => {
    const user = userEvent.setup();
    const { $api } = await import('../../api/client');
    render(
      <InputPickerModal
        strip={snare}
        strips={console_}
        open
        onClose={() => {}}
        onPatch={() => {}}
      />,
    );
    await screen.findByText('dock');
    const callsAfterOpen = vi.mocked($api.POST).mock.calls.length;
    await user.click(
      screen.getByRole('button', { name: 'Rescan audio devices and retry failed ones' }),
    );
    expect(vi.mocked($api.POST).mock.calls.length).toBe(callsAfterOpen + 1);
  });

  describe('the card profile picker', () => {
    const umc = (overrides = {}) => ({
      name: 'alsa_input.usb-UMC1820.analog-stereo',
      label: 'UMC1820',
      channels: 2,
      active: false,
      status: 'available' as const,
      patched: false,
      underruns: 0,
      overruns: 0,
      card: 'alsa_card.usb-UMC1820',
      profile: 'input:analog-stereo',
      profiles: [
        { name: 'input:analog-stereo', description: 'Analog Stereo Input' },
        { name: 'pro-audio', description: 'Pro Audio' },
      ],
      reconciled_from: null,
      ...overrides,
    });

    async function openWith(devices: unknown[]) {
      const { $api } = await import('../../api/client');
      vi.mocked($api.POST).mockResolvedValue({ data: devices } as never);
      render(
        <InputPickerModal
          strip={snare}
          strips={console_}
          open
          onClose={() => {}}
          onPatch={() => {}}
        />,
      );
      await screen.findByText('UMC1820');
      return $api;
    }

    it('names the profile that reveals the missing inputs', async () => {
      await openWith([umc()]);
      const select = screen.getByLabelText('Mode');
      expect(select).toHaveValue('input:analog-stereo');
      // pactl publishes no per-profile channel count, so the name is the
      // only honest guidance we can give.
      expect(
        screen.getByRole('option', { name: 'Pro Audio (all inputs)' }),
      ).toBeInTheDocument();
      expect(screen.getByText('2 in')).toBeInTheDocument();
    });

    it('sends the change against the CARD, not the device', async () => {
      const user = userEvent.setup();
      const $api = await openWith([umc()]);
      await user.selectOptions(screen.getByLabelText('Mode'), 'pro-audio');
      expect(vi.mocked($api.PUT)).toHaveBeenCalledWith('/api/v1/devices/profile', {
        body: { card: 'alsa_card.usb-UMC1820', profile: 'pro-audio' },
      });
    });

    it('surfaces a refused switch instead of silently reverting', async () => {
      const user = userEvent.setup();
      const $api = await openWith([umc()]);
      vi.mocked($api.PUT).mockResolvedValue({ error: 'card is busy' } as never);
      await user.selectOptions(screen.getByLabelText('Mode'), 'pro-audio');
      expect(await screen.findByRole('alert')).toHaveTextContent('card is busy');
    });

    it('stays out of the way when the card offers no alternative', async () => {
      await openWith([umc({ profiles: [] })]);
      expect(screen.queryByLabelText('Mode')).not.toBeInTheDocument();
    });
  });
});
