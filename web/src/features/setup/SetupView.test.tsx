import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { $api } from '../../api/client';
import { useSettings } from '../../state/settings';
import { useTransport } from '../../state/transport';
import { SetupView } from './SetupView';

vi.mock('../../api/client', () => ({
  $api: { GET: vi.fn(), PUT: vi.fn() },
}));

const settingsDto = (over: Record<string, unknown> = {}) => ({
  destination: '/home/user/projects',
  configured_destination: null,
  default_destination: '/home/user/projects',
  project_name: 'Session',
  format: 'wav32_float',
  active_sample_rate: 48_000,
  configured_sample_rate: 48_000,
  restart_required: false,
  ...over,
});

const destinationsDto = {
  current: '/home/user/projects',
  default: '/home/user/projects',
  drives: [
    {
      mount_point: '/run/media/user/STICK',
      label: 'STICK',
      total_bytes: 32_000_000_000,
      available_bytes: 14_200_000_000,
      removable: true,
      read_only: false,
    },
    {
      mount_point: '/run/media/user/LOCKED',
      label: 'LOCKED',
      total_bytes: 8_000_000_000,
      available_bytes: 1_000_000_000,
      removable: true,
      read_only: true,
    },
  ],
};

function mockGets(settings: Record<string, unknown> = {}) {
  vi.mocked($api.GET).mockImplementation((path: string) =>
    Promise.resolve(
      path === '/api/v1/settings/recording'
        ? { data: settingsDto(settings) }
        : { data: destinationsDto },
    ) as never,
  );
}

function transport(over: Record<string, unknown> = {}) {
  useTransport.setState({ phase: 'stopped', sampleRate: 48_000, ...over });
}

beforeEach(() => {
  vi.mocked($api.GET).mockReset();
  vi.mocked($api.PUT).mockReset();
  useSettings.setState({
    settings: null,
    confirmed: null,
    destinations: [],
    loaded: false,
    error: null,
  });
  transport();
  mockGets();
});

describe('SetupView', () => {
  it('renders the three panels with drive tiles, free space, and status lamps', async () => {
    render(<SetupView />);
    expect(await screen.findByRole('heading', { name: 'Destination' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'File format' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Sample rate' })).toBeTruthy();

    const stick = await screen.findByRole('option', { name: /STICK/ });
    expect(stick.textContent).toContain('14.2 GB free');
    expect(stick.textContent).toContain('ready');
    const locked = screen.getByRole('option', { name: /LOCKED/ });
    expect(locked.textContent).toContain('read-only');
    expect((locked as HTMLButtonElement).disabled).toBe(true);

    const internal = screen.getByRole('option', { name: /Internal/ });
    expect(internal.getAttribute('aria-selected')).toBe('true');
  });

  it('clicking a drive tile PUTs its tributary subdir as the destination', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: settingsDto({ destination: '/run/media/user/STICK/tributary' }),
      response: { status: 200 },
    } as never);
    render(<SetupView />);
    await userEvent.click(await screen.findByRole('option', { name: /STICK/ }));
    expect($api.PUT).toHaveBeenCalledWith('/api/v1/settings/recording', {
      body: {
        destination: '/run/media/user/STICK/tributary',
        format: undefined,
        sample_rate: undefined,
      },
    });
  });

  it('a 422 rolls back and shows the daemon detail inline', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: undefined,
      error: { error: 'invalid', detail: '/run/media/user/STICK/tributary is not writable' },
      response: { status: 422 },
    } as never);
    render(<SetupView />);
    await userEvent.click(await screen.findByRole('option', { name: /STICK/ }));
    expect(
      await screen.findByText('/run/media/user/STICK/tributary is not writable'),
    ).toBeTruthy();
    expect(useSettings.getState().settings?.destination).toBe('/home/user/projects');
  });

  it('recording locks the destination but not format or rate', async () => {
    transport({ phase: 'recording' });
    render(<SetupView />);
    const stick = await screen.findByRole('option', { name: /STICK/ });
    expect((stick as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText('destination locked while recording')).toBeTruthy();
    const wav16 = screen.getByRole('radio', { name: 'WAV 16' });
    expect((wav16 as HTMLButtonElement).disabled).toBe(false);
    const rate96 = screen.getByRole('radio', { name: '96 kHz' });
    expect((rate96 as HTMLButtonElement).disabled).toBe(false);
  });

  it('shows the restart lamp when the configured rate differs from the engine', async () => {
    mockGets({ configured_sample_rate: 96_000, restart_required: true });
    render(<SetupView />);
    expect(await screen.findByText('Restart required')).toBeTruthy();
  });

  it('hides the restart lamp when configured matches active', async () => {
    render(<SetupView />);
    await screen.findByRole('option', { name: /STICK/ });
    expect(screen.queryByText('Restart required')).toBeNull();
  });

  it('a format press PUTs the new format', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: settingsDto({ format: 'flac24' }),
      response: { status: 200 },
    } as never);
    render(<SetupView />);
    await userEvent.click(await screen.findByRole('radio', { name: 'FLAC 24' }));
    expect($api.PUT).toHaveBeenCalledWith('/api/v1/settings/recording', {
      body: { destination: undefined, format: 'flac24', sample_rate: undefined },
    });
    expect(screen.getByRole('radio', { name: 'FLAC 24' }).getAttribute('aria-checked')).toBe(
      'true',
    );
  });

  it('the REC PATH lamp goes red after a boot fallback', async () => {
    mockGets({
      destination: '/home/user/projects',
      configured_destination: '/run/media/user/GONE/tributary',
    });
    render(<SetupView />);
    expect(await screen.findByText(/configured drive was missing/)).toBeTruthy();
    expect(screen.getByText('missing')).toBeTruthy();
  });
});
