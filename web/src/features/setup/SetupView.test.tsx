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

// The live drive feed opens a socket; these tests assert rendering off the
// REST seed, which the hook performs either way.
vi.mock('../../ws/client-instance', () => ({
  wsClient: { subscribe: () => () => {}, onMessage: () => () => {} },
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
      device: '/dev/sdb1',
      disk: '/dev/sdb',
      mount_point: '/run/media/user/STICK',
      label: 'STICK',
      filesystem: 'exfat',
      total_bytes: 32_000_000_000,
      available_bytes: 14_200_000_000,
      removable: true,
      transport: 'usb',
      state: 'ready',
      reason: null,
    },
    {
      device: '/dev/sdc1',
      disk: '/dev/sdc',
      mount_point: '/run/media/user/LOCKED',
      label: 'LOCKED',
      filesystem: 'ext4',
      total_bytes: 8_000_000_000,
      available_bytes: 1_000_000_000,
      removable: true,
      transport: 'usb',
      state: 'read_only',
      reason: 'mounted read-only',
    },
  ],
};

const sessionsDto = {
  root: '/home/user/projects',
  root_present: true,
  sessions: [
    {
      id: '1786380405-session',
      name: 'Session',
      created_at_unix: 1_786_380_405,
      take_count: 2,
      open: true,
    },
  ],
};

function mockGets(settings: Record<string, unknown> = {}) {
  vi.mocked($api.GET).mockImplementation((path: string) => {
    if (path === '/api/v1/settings/recording') return Promise.resolve({ data: settingsDto(settings) }) as never;
    if (path === '/api/v1/sessions') return Promise.resolve({ data: sessionsDto }) as never;
    return Promise.resolve({ data: destinationsDto }) as never;
  });
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
    expect(locked.textContent).toContain('mounted read-only');
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

  /**
   * The reported bug, encoded. A drive is physically connected but nothing
   * on it can be recorded to; before this change the panel rendered
   * nothing at all, which is indistinguishable from an empty USB port.
   * Every volume must stay on screen with a reason — MASTER.md: "nothing
   * here folds — drive status must never be hidden."
   */
  it('a connected but unusable drive is shown with a reason, never as an empty port', async () => {
    vi.mocked($api.GET).mockImplementation((path: string) =>
      Promise.resolve(
        path === '/api/v1/sessions'
          ? { data: sessionsDto }
          : path === '/api/v1/settings/recording'
          ? { data: settingsDto() }
          : {
              data: {
                current: '/home/user/projects',
                default: '/home/user/projects',
                drives: [
                  {
                    device: '/dev/sda1',
                    disk: '/dev/sda',
                    mount_point: '/media/bootfs',
                    label: 'bootfs',
                    filesystem: 'vfat',
                    total_bytes: 536_870_912,
                    available_bytes: 450_450_944,
                    removable: true,
                    transport: 'usb',
                    state: 'too_small',
                    reason: 'too small to record onto',
                  },
                  {
                    device: '/dev/sda2',
                    disk: '/dev/sda',
                    mount_point: null,
                    label: 'rootfs',
                    filesystem: 'ext4',
                    total_bytes: 3_238_002_688,
                    available_bytes: null,
                    removable: true,
                    transport: 'usb',
                    state: 'not_mounted',
                    reason: 'connected, but nothing mounted it',
                  },
                ],
              },
            },
      ) as never,
    );
    render(<SetupView />);

    // Both volumes are on screen, each saying why it cannot be used.
    expect(await screen.findByRole('option', { name: /bootfs/ })).toBeTruthy();
    expect(screen.getByText('too small to record onto')).toBeTruthy();
    expect(screen.getByText('connected, but nothing mounted it')).toBeTruthy();
    // Neither is selectable, and the drive says so once at the top.
    expect(
      (screen.getByRole('option', { name: /bootfs/ }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText('no volume on this drive can be recorded to')).toBeTruthy();
    // And this must NOT read as "no drive connected".
    expect(screen.queryByText(/No USB drive connected/)).toBeNull();
  });

  it('says so plainly when there really is no drive', async () => {
    vi.mocked($api.GET).mockImplementation((path: string) =>
      Promise.resolve(
        path === '/api/v1/sessions'
          ? { data: sessionsDto }
          : path === '/api/v1/settings/recording'
          ? { data: settingsDto() }
          : { data: { current: '/p', default: '/p', drives: [] } },
      ) as never,
    );
    render(<SetupView />);
    expect(await screen.findByText(/No drive connected/)).toBeTruthy();
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
