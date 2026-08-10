/**
 * The daemon's device document: input devices joined with patch state.
 * Loaded at boot, replaced wholesale by the patchbay's open/Refresh (which
 * also retries failed devices daemon-side).
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';

export type DeviceReport = components['schemas']['DeviceReport'];

export interface DevicesState {
  devices: DeviceReport[];
  loaded: boolean;
  set: (devices: DeviceReport[]) => void;
}

export const useDevices = create<DevicesState>((set) => ({
  devices: [],
  loaded: false,
  set: (devices) => set({ devices, loaded: true }),
}));

/** Side-effect-free read. */
export async function loadDevices(): Promise<void> {
  const { data } = await $api.GET('/api/v1/devices');
  if (data) useDevices.getState().set(data);
}

/** The Refresh path: re-enumerate AND retry wanted devices daemon-side. */
export async function refreshDevices(): Promise<void> {
  const { data } = await $api.POST('/api/v1/devices/refresh');
  if (data) useDevices.getState().set(data);
}
