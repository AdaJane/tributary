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
  /** The card whose profile change is in flight — its streams are down
   * while the server renegotiates, which takes a visible moment. */
  pendingCard: string | null;
  set: (devices: DeviceReport[]) => void;
  setPending: (card: string | null) => void;
}

export const useDevices = create<DevicesState>((set) => ({
  devices: [],
  loaded: false,
  pendingCard: null,
  set: (devices) => set({ devices, loaded: true }),
  setPending: (pendingCard) => set({ pendingCard }),
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

/**
 * Switch a sound card's profile — how a device with more inputs than the
 * current profile exposes gets the rest of them.
 *
 * Deliberately NOT optimistic: the switch renames, resizes and remaps the
 * card's devices, so there is no honest way to predict the result. The
 * daemon returns the whole new document and we adopt it. Resolves to an
 * error string the caller shows inline, or null on success.
 */
export async function setCardProfile(card: string, profile: string): Promise<string | null> {
  useDevices.getState().setPending(card);
  const { data, error } = await $api.PUT('/api/v1/devices/profile', {
    body: { card, profile },
  });
  useDevices.getState().setPending(null);
  if (data) {
    useDevices.getState().set(data);
    return null;
  }
  return typeof error === 'string' ? error : 'the profile change was refused';
}
