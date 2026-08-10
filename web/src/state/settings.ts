/**
 * The daemon's recording settings, mirrored with optimistic apply and
 * rollback. Unlike the fire-and-forget console gestures, settings PUTs are
 * awaited: a refused save must roll the control back and say why. The
 * daemon owns this state — nothing here persists client-side.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';
import { saveErrorMessage } from '../features/setup/settings-logic';

export type RecordingFormat = components['schemas']['RecordFormat'];
type SettingsDto = components['schemas']['RecordingSettingsDto'];
type DestinationsDto = components['schemas']['DestinationsDto'];

export interface RecordingSettings {
  /** Where the next take lands. */
  destination: string;
  /** The persisted pref — differs from `destination` after a boot fallback. */
  configuredDestination: string | null;
  defaultDestination: string;
  projectName: string;
  format: RecordingFormat;
  activeSampleRate: number;
  configuredSampleRate: number;
  restartRequired: boolean;
}

export interface Destination {
  mountPath: string;
  label: string;
  totalBytes: number;
  freeBytes: number;
  removable: boolean;
  writable: boolean;
}

export type SettingsField = 'destination' | 'format' | 'sampleRate';

export interface SettingsError {
  field: SettingsField;
  message: string;
}

export interface SettingsState {
  /** What the controls show — optimistic during an in-flight save. */
  settings: RecordingSettings | null;
  /** The last server-acked truth — the rollback point. */
  confirmed: RecordingSettings | null;
  destinations: Destination[];
  loaded: boolean;
  error: SettingsError | null;
  confirm: (settings: RecordingSettings) => void;
  applyOptimistic: (patch: Partial<RecordingSettings>) => void;
  fail: (error: SettingsError) => void;
  setDestinations: (destinations: Destination[]) => void;
}

export const useSettings = create<SettingsState>((set) => ({
  settings: null,
  confirmed: null,
  destinations: [],
  loaded: false,
  error: null,
  confirm: (settings) => set({ settings, confirmed: settings, loaded: true, error: null }),
  applyOptimistic: (patch) =>
    set((s) => ({
      settings: s.settings === null ? null : { ...s.settings, ...patch },
      error: null,
    })),
  fail: (error) => set((s) => ({ settings: s.confirmed, error })),
  setDestinations: (destinations) => set({ destinations }),
}));

const fromSettingsDto = (dto: SettingsDto): RecordingSettings => ({
  destination: dto.destination,
  configuredDestination: dto.configured_destination ?? null,
  defaultDestination: dto.default_destination,
  projectName: dto.project_name,
  format: dto.format,
  activeSampleRate: dto.active_sample_rate,
  configuredSampleRate: dto.configured_sample_rate,
  restartRequired: dto.restart_required,
});

const fromDestinationsDto = (dto: DestinationsDto): Destination[] =>
  dto.drives.map((d) => ({
    mountPath: d.mount_point,
    label: d.label,
    totalBytes: d.total_bytes,
    freeBytes: d.available_bytes,
    removable: d.removable,
    writable: !d.read_only,
  }));

/** Side-effect-free read. */
export async function loadSettings(): Promise<void> {
  const { data } = await $api.GET('/api/v1/settings/recording');
  if (data) useSettings.getState().confirm(fromSettingsDto(data));
}

/** Fresh enumeration on every call — the Rescan button just re-GETs. */
export async function loadDestinations(): Promise<void> {
  const { data } = await $api.GET('/api/v1/destinations');
  if (data) useSettings.getState().setDestinations(fromDestinationsDto(data));
}

export interface SettingsPatch {
  destination?: string;
  format?: RecordingFormat;
  sampleRate?: number;
}

/**
 * Apply one setting: optimistic update, then the PUT; the response body
 * confirms, a refusal rolls back and surfaces an inline error on `field`.
 */
export async function saveSettings(patch: SettingsPatch, field: SettingsField): Promise<void> {
  const store = useSettings.getState();
  store.applyOptimistic({
    ...(patch.destination !== undefined && { destination: patch.destination }),
    ...(patch.format !== undefined && { format: patch.format }),
    ...(patch.sampleRate !== undefined && { configuredSampleRate: patch.sampleRate }),
  });
  try {
    const { data, error, response } = await $api.PUT('/api/v1/settings/recording', {
      body: {
        destination: patch.destination,
        format: patch.format,
        sample_rate: patch.sampleRate,
      },
    });
    if (data) {
      useSettings.getState().confirm(fromSettingsDto(data));
    } else {
      const detail = (error as { detail?: string } | undefined)?.detail;
      useSettings.getState().fail({
        field,
        message: saveErrorMessage(response.status, detail),
      });
    }
  } catch {
    useSettings.getState().fail({ field, message: saveErrorMessage('network') });
  }
}
