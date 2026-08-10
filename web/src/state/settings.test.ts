import { beforeEach, describe, expect, it, vi } from 'vitest';

import { loadDestinations, loadSettings, saveSettings, useSettings } from './settings';
import { $api } from '../api/client';

vi.mock('../api/client', () => ({
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
});

describe('loadSettings', () => {
  it('populates the optimistic and confirmed views together', async () => {
    vi.mocked($api.GET).mockResolvedValue({ data: settingsDto() } as never);
    await loadSettings();
    const s = useSettings.getState();
    expect(s.loaded).toBe(true);
    expect(s.settings?.destination).toBe('/home/user/projects');
    expect(s.settings).toEqual(s.confirmed);
  });
});

describe('loadDestinations', () => {
  it('maps drives, inverting read_only into writable', async () => {
    vi.mocked($api.GET).mockResolvedValue({
      data: {
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
        ],
      },
    } as never);
    await loadDestinations();
    expect(useSettings.getState().destinations).toEqual([
      {
        mountPath: '/run/media/user/STICK',
        label: 'STICK',
        totalBytes: 32_000_000_000,
        freeBytes: 14_200_000_000,
        removable: true,
        writable: true,
      },
    ]);
  });
});

describe('saveSettings', () => {
  beforeEach(async () => {
    vi.mocked($api.GET).mockResolvedValue({ data: settingsDto() } as never);
    await loadSettings();
  });

  it('applies optimistically, then confirms from the response body', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: settingsDto({ format: 'flac24' }),
      response: { status: 200 },
    } as never);
    await saveSettings({ format: 'flac24' }, 'format');
    const s = useSettings.getState();
    expect(s.settings?.format).toBe('flac24');
    expect(s.confirmed?.format).toBe('flac24');
    expect(s.error).toBeNull();
  });

  it('rolls back to confirmed and surfaces the 409 as locked-while-recording', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: undefined,
      error: { error: 'conflict', detail: 'stop recording first' },
      response: { status: 409 },
    } as never);
    await saveSettings({ destination: '/run/media/user/STICK' }, 'destination');
    const s = useSettings.getState();
    expect(s.settings?.destination, 'rolled back').toBe('/home/user/projects');
    expect(s.error).toEqual({ field: 'destination', message: 'locked while recording' });
  });

  it('surfaces the 422 detail verbatim', async () => {
    vi.mocked($api.PUT).mockResolvedValue({
      data: undefined,
      error: { error: 'invalid', detail: '/mnt/x is not writable' },
      response: { status: 422 },
    } as never);
    await saveSettings({ destination: '/mnt/x' }, 'destination');
    expect(useSettings.getState().error).toEqual({
      field: 'destination',
      message: '/mnt/x is not writable',
    });
  });

  it('treats a thrown fetch as daemon-unreachable and rolls back', async () => {
    vi.mocked($api.PUT).mockRejectedValue(new TypeError('fetch failed'));
    await saveSettings({ sampleRate: 96_000 }, 'sampleRate');
    const s = useSettings.getState();
    expect(s.settings?.configuredSampleRate).toBe(48_000);
    expect(s.error).toEqual({ field: 'sampleRate', message: 'daemon unreachable' });
  });

  it('a later successful save clears the error', async () => {
    vi.mocked($api.PUT).mockResolvedValueOnce({
      data: undefined,
      error: { error: 'conflict', detail: 'x' },
      response: { status: 409 },
    } as never);
    await saveSettings({ format: 'flac16' }, 'format');
    expect(useSettings.getState().error).not.toBeNull();
    vi.mocked($api.PUT).mockResolvedValueOnce({
      data: settingsDto({ format: 'flac16' }),
      response: { status: 200 },
    } as never);
    await saveSettings({ format: 'flac16' }, 'format');
    expect(useSettings.getState().error).toBeNull();
  });
});
