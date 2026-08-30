/**
 * The daemon's recording settings, mirrored with optimistic apply and
 * rollback. Unlike the fire-and-forget console gestures, settings PUTs are
 * awaited: a refused save must roll the control back and say why. The
 * daemon owns this state — nothing here persists client-side.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';
import type { Transport } from '../features/setup/drives-logic';
import type { Filesystem } from '../features/setup/format-logic';
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

export type DriveState = components['schemas']['DriveState'];

export interface Destination {
  /** `null` when the drive is present but nothing mounted it. */
  mountPath: string | null;
  device: string | null;
  label: string;
  filesystem: string | null;
  /** How it is attached — an icon and a printed word, never a permission. */
  transport: Transport;
  totalBytes: number;
  /** `null` until it is mounted — free space is a statvfs answer. */
  freeBytes: number | null;
  removable: boolean;
  state: DriveState;
  /** Why this cannot be recorded to; `null` when it can. */
  reason: string | null;
  /** The whole disk it lives on, for grouping under one drive. */
  disk: string | null;
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
  /** Whether this installation ships the privileged storage helper. False
   *  on package installs; the Format control is disabled with a reason. */
  canFormat: boolean;
  loaded: boolean;
  error: SettingsError | null;
  confirm: (settings: RecordingSettings) => void;
  applyOptimistic: (patch: Partial<RecordingSettings>) => void;
  fail: (error: SettingsError) => void;
  setDestinations: (destinations: Destination[]) => void;
  setCanFormat: (canFormat: boolean) => void;
}

export const useSettings = create<SettingsState>((set) => ({
  settings: null,
  confirmed: null,
  destinations: [],
  canFormat: false,
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
  setCanFormat: (canFormat) => set({ canFormat }),
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

type DriveDto = DestinationsDto['drives'][number];

const fromDrives = (drives: readonly DriveDto[]): Destination[] =>
  drives.map((d) => ({
    mountPath: d.mount_point ?? null,
    device: d.device ?? null,
    label: d.label,
    filesystem: d.filesystem ?? null,
    totalBytes: d.total_bytes,
    freeBytes: d.available_bytes ?? null,
    removable: d.removable,
    transport: d.transport,
    state: d.state,
    reason: d.reason ?? null,
    disk: d.disk ?? null,
  }));

/** Side-effect-free read. */
export async function loadSettings(): Promise<void> {
  const { data } = await $api.GET('/api/v1/settings/recording');
  if (data) useSettings.getState().confirm(fromSettingsDto(data));
}

/** Fresh enumeration on every call — the Rescan button just re-GETs. */
export async function loadDestinations(): Promise<void> {
  const { data } = await $api.GET('/api/v1/destinations');
  if (data) {
    useSettings.getState().setDestinations(fromDrives(data.drives));
    useSettings.getState().setCanFormat(data.can_format);
  }
}

/** A pushed drive list from the `destinations` channel. Same mapping as
 *  the GET, so a hotplug update and a Rescan cannot disagree. */
export function applyDestinations(drives: readonly DriveDto[]): void {
  useSettings.getState().setDestinations(fromDrives(drives));
}

/**
 * Wipe a drive and lay down one volume. Returns `null` on success, else a
 * sentence to show. Not optimistic: there is no honest way to predict what
 * a drive becomes, and the confirmation is the pushed `destinations`
 * update showing the reformatted drive — the same idiom `setCardProfile`
 * uses for a card-profile switch.
 */
export async function formatDrive(
  device: string,
  label: string,
  filesystem: Filesystem,
): Promise<string | null> {
  const { formatErrorMessage } = await import('../features/setup/format-logic');
  try {
    const { error, response } = await $api.POST('/api/v1/destinations/format', {
      body: { device, label, filesystem },
    });
    if (!error) return null;
    const detail = (error as { detail?: string } | undefined)?.detail;
    return formatErrorMessage(response.status, detail);
  } catch {
    return formatErrorMessage('network');
  }
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
