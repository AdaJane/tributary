import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useDevices } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import { InputPickerModal } from './InputPickerModal';

// The modal's open/Refresh hits POST /api/v1/devices/refresh; feed it a
// two-box system unless a test overrides the mock.
vi.mock('../../api/client', () => ({
  $api: {
    GET: vi.fn().mockResolvedValue({ data: undefined }),
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
    expect(onPatch).toHaveBeenCalledWith({ device: 'dock', channel: 0 });
    await user.click(
      screen.getByRole('option', { name: 'IN 1 on System default input, in use by Kick' }),
    );
    expect(onPatch).toHaveBeenCalledWith({ device: null, channel: 0 });
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
});
