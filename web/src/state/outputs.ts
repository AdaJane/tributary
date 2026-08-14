/**
 * The daemon's output patch bay document: patches joined with the devices
 * they aim at.
 *
 * Deliberately NOT optimistic, for the reason `setCardProfile` gives:
 * patching opens a device, which can fail, resize, or turn out to be muted
 * — there is no honest way to predict the result, so the daemon returns
 * the whole new document and we adopt it.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';

export type OutputsDto = components['schemas']['OutputsDto'];
export type OutputPatchRequest = components['schemas']['OutputPatchRequest'];

/** Stable empties: a selector returning a fresh `[]` re-renders for ever. */
const NO_PATCHES: OutputsDto['patches'] = [];
const NO_REPORTS: OutputsDto['reports'] = [];
const NO_DEVICES: OutputsDto['devices'] = [];

export interface OutputsState {
  document: OutputsDto | null;
  loaded: boolean;
  /** Set while a patch is in flight; the room disables while it is. */
  pending: boolean;
  set: (document: OutputsDto) => void;
  setPending: (pending: boolean) => void;
}

export const useOutputs = create<OutputsState>((set) => ({
  document: null,
  loaded: false,
  pending: false,
  set: (document) => set({ document, loaded: true, pending: false }),
  setPending: (pending) => set({ pending }),
}));

export const outputPatches = (state: OutputsState) => state.document?.patches ?? NO_PATCHES;
export const outputReports = (state: OutputsState) => state.document?.reports ?? NO_REPORTS;
export const outputDevices = (state: OutputsState) => state.document?.devices ?? NO_DEVICES;

/** Side-effect-free read. */
export async function loadOutputs(): Promise<void> {
  const { data } = await $api.GET('/api/v1/outputs');
  if (data) useOutputs.getState().set(data);
}

/** Re-enumerate AND retry wanted-but-unopened outputs daemon-side. */
export async function refreshOutputs(): Promise<void> {
  const { data } = await $api.POST('/api/v1/outputs/refresh');
  if (data) useOutputs.getState().set(data);
}

/** Wording for a refused patch, in the console's voice. */
export function patchErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 409) return 'locked while recording';
  if (status === 501) return 'this audio backend has no patchable outputs';
  return detail ?? 'the patch was refused';
}

/**
 * Apply one change. Resolves to an error sentence the caller shows inline,
 * or null on success.
 */
export async function patchOutput(body: OutputPatchRequest): Promise<string | null> {
  useOutputs.getState().setPending(true);
  try {
    const { data, error, response } = await $api.PUT('/api/v1/outputs/patch', { body });
    if (data) {
      useOutputs.getState().set(data);
      return null;
    }
    useOutputs.getState().setPending(false);
    const detail = (error as { detail?: string } | undefined)?.detail;
    return patchErrorMessage(response.status, detail);
  } catch {
    useOutputs.getState().setPending(false);
    return patchErrorMessage('network');
  }
}
