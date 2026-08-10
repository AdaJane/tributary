import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useTransport } from '../../state/transport';
import type { StripState } from '../../ws/messages';
import { ChannelStrip } from './ChannelStrip';

vi.mock('../../api/client', () => ({
  $api: {
    GET: vi.fn().mockResolvedValue({ data: undefined }),
    POST: vi.fn().mockResolvedValue({ data: undefined }),
    PUT: vi.fn().mockResolvedValue({ data: undefined }),
    DELETE: vi.fn().mockResolvedValue({ data: undefined }),
  },
  API_BASE_URL: 'http://test',
  WS_URL: 'ws://test/ws',
  MONITOR_WS_URL: 'ws://test/ws/monitor',
}));
import { $api } from '../../api/client';

const strip: StripState = {
  id: 2,
  name: 'Snare',
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
  fader_db: -90,
  mute: false,
  pfl: false,
  record_arm: false,
  route_to: { kind: 'master' },
};

beforeEach(() => {
  vi.mocked($api.DELETE).mockClear();
  useTransport.setState({ phase: 'stopped' });
});

describe('removing a channel', () => {
  it('takes two clicks — arm, then confirm', async () => {
    const user = userEvent.setup();
    render(<ChannelStrip strip={strip} />);
    const remove = screen.getByRole('button', { name: 'Remove channel Snare' });
    await user.click(remove);
    expect($api.DELETE).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Confirm removing Snare' }));
    expect($api.DELETE).toHaveBeenCalledWith('/api/v1/strips/{id}', {
      params: { path: { id: 2 } },
    });
  });

  it('is disabled while tape rolls — the layout is frozen', () => {
    useTransport.setState({ phase: 'recording' });
    render(<ChannelStrip strip={strip} />);
    expect(
      screen.getByRole('button', { name: 'Remove Snare (stop recording first)' }),
    ).toBeDisabled();
  });
});
