import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { $api } from '../../api/client';
import { useTakes } from '../../state/takes';
import { useTransport } from '../../state/transport';
import { TakeBrowserModal } from './TakeBrowserModal';

vi.mock('../../api/client', () => ({
  $api: { GET: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
}));

const take = (over: Partial<ReturnType<typeof base>> = {}) => ({ ...base(), ...over });

function base() {
  return {
    take: 3,
    startedAtUnix: 1_786_380_405,
    sampleRate: 48_000,
    damaged: false,
    durationSecs: 187,
    tracks: [
      { file: 'ch01-kick.wav', channels: 1, stripId: 0, damaged: false },
      { file: 'master.wav', channels: 2, stripId: null, damaged: false },
    ],
  };
}

function seed(over: { takes?: ReturnType<typeof base>[]; phase?: string; take?: number } = {}) {
  useTakes.setState({
    takes: over.takes ?? [take(), take({ take: 2, durationSecs: 61 }), take({ take: 1 })],
    status: 'ready',
    error: null,
    pending: null,
  });
  useTransport.setState({
    phase: (over.phase ?? 'stopped') as never,
    take: over.take ?? 3,
    latestTake: 3,
    engineSampleRate: 48_000,
  });
}

beforeEach(() => {
  vi.mocked($api.PUT).mockReset();
  vi.mocked($api.DELETE).mockReset();
  seed();
});

describe('TakeBrowserModal', () => {
  it('lists every take newest first, with time, duration and track count', async () => {
    render(<TakeBrowserModal open onClose={() => {}} />);
    const rows = await screen.findAllByRole('option');
    expect(rows).toHaveLength(3);
    expect(rows[0].textContent).toContain('T03');
    expect(rows[0].textContent).toContain('3:07');
    expect(rows[0].textContent).toContain('2 tracks');
    expect(rows[1].textContent).toContain('T02');
    expect(rows[2].textContent).toContain('T01');
  });

  it('marks the selected take', async () => {
    seed({ take: 2 });
    render(<TakeBrowserModal open onClose={() => {}} />);
    const rows = await screen.findAllByRole('option');
    expect(rows[1].getAttribute('aria-selected')).toBe('true');
    expect(rows[0].getAttribute('aria-selected')).toBe('false');
  });

  it('picking a row PUTs the selection and closes', async () => {
    vi.mocked($api.PUT).mockResolvedValue({ data: {}, response: { status: 200 } } as never);
    const onClose = vi.fn();
    render(<TakeBrowserModal open onClose={onClose} />);
    await userEvent.click(await screen.findByRole('option', { name: /T01/ }));
    expect($api.PUT).toHaveBeenCalledWith('/api/v1/transport/take', { body: { take: 1 } });
  });

  it('shows the daemon refusal inline instead of failing silently', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: undefined,
      error: { error: 'conflict', detail: 'no take switching while recording' },
      response: { status: 409 },
    } as never);
    render(<TakeBrowserModal open onClose={() => {}} />);
    await userEvent.click(await screen.findByRole('option', { name: /T01/ }));
    expect(await screen.findByText('no take switching while recording')).toBeTruthy();
  });

  /**
   * A take cut at another rate is still real audio with a real waveform.
   * Refusing to show it would make it indistinguishable from a take that
   * isn't there — so it stays selectable and says why PLAY won't roll it.
   */
  it('keeps a rate-mismatched take selectable and prints why it will not play', async () => {
    seed({ takes: [take({ sampleRate: 44_100 })] });
    render(<TakeBrowserModal open onClose={() => {}} />);
    const row = await screen.findByRole('option', { name: /T03/ });
    expect((row as HTMLButtonElement).disabled).toBe(false);
    expect(row.textContent).toContain('cut at 44.1 kHz · the engine runs 48 kHz');
  });

  it('flags a damaged take', async () => {
    seed({ takes: [take({ damaged: true })] });
    render(<TakeBrowserModal open onClose={() => {}} />);
    expect((await screen.findByRole('option', { name: /T03/ })).textContent).toContain('DAMAGED');
  });

  it('disables every row while tape is rolling, and says why', async () => {
    seed({ phase: 'recording' });
    render(<TakeBrowserModal open onClose={() => {}} />);
    const rows = await screen.findAllByRole('option');
    expect(rows.every((r) => (r as HTMLButtonElement).disabled)).toBe(true);
    expect(screen.getByText(/tape is rolling/)).toBeTruthy();
  });

  /** Two steps, and the confirm names the take — no per-row ✕ to
   *  fat-finger on a phone. */
  it('deletes only after a second, explicit press', async () => {
    vi.mocked($api.DELETE).mockResolvedValue({ data: [], response: { status: 200 } } as never);
    render(<TakeBrowserModal open onClose={() => {}} />);
    await userEvent.click(await screen.findByRole('button', { name: /Delete T03/ }));
    expect($api.DELETE).not.toHaveBeenCalled();
    expect(screen.getByText(/cannot be undone/)).toBeTruthy();

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect($api.DELETE).toHaveBeenCalledWith('/api/v1/takes/{take}', {
      params: { path: { take: 3 } },
    });
  });

  it('says so plainly when the session has no takes', async () => {
    seed({ takes: [] });
    render(<TakeBrowserModal open onClose={() => {}} />);
    expect(await screen.findByText(/No takes in this session yet/)).toBeTruthy();
  });
});
