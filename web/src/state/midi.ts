/**
 * The daemon's MIDI patch bay document: routes joined with both
 * directions' ports and the selected take's tracks.
 *
 * A second store beside `outputs.ts` rather than a field on it, because
 * the two documents are refreshed by different things — re-enumerating
 * audio devices tells you nothing about a sequencer port — and one store
 * would make every MIDI edit re-render the audio bay.
 *
 * Non-optimistic for the same reason: routing opens a port, which can
 * fail, so the daemon returns the whole new document and we adopt it.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';

export type MidiDto = components['schemas']['MidiDto'];
export type MidiRouteRequest = components['schemas']['MidiRouteRequest'];

/** Stable empties: a selector returning a fresh `[]` re-renders for ever. */
const NO_ROUTES: MidiDto['routes'] = [];
const NO_REPORTS: MidiDto['reports'] = [];
const NO_OUT_PORTS: MidiDto['outputs'] = [];
const NO_IN_PORTS: MidiDto['inputs'] = [];
const NO_TRACKS: MidiDto['take_tracks'] = [];

export interface MidiState {
  document: MidiDto | null;
  loaded: boolean;
  /** Set while a route change is in flight; the room disables while it is. */
  pending: boolean;
  set: (document: MidiDto) => void;
  setPending: (pending: boolean) => void;
}

export const useMidi = create<MidiState>((set) => ({
  document: null,
  loaded: false,
  pending: false,
  set: (document) => set({ document, loaded: true, pending: false }),
  setPending: (pending) => set({ pending }),
}));

export const midiRoutes = (state: MidiState) => state.document?.routes ?? NO_ROUTES;
export const midiReports = (state: MidiState) => state.document?.reports ?? NO_REPORTS;
export const midiOutPorts = (state: MidiState) => state.document?.outputs ?? NO_OUT_PORTS;
export const midiInPorts = (state: MidiState) => state.document?.inputs ?? NO_IN_PORTS;
export const midiTakeTracks = (state: MidiState) => state.document?.take_tracks ?? NO_TRACKS;

/** Side-effect-free read. */
export async function loadMidi(): Promise<void> {
  const { data } = await $api.GET('/api/v1/midi');
  if (data) useMidi.getState().set(data);
}

/** Re-enumerate both directions AND retry anything that failed to open. */
export async function refreshMidi(): Promise<void> {
  const { data } = await $api.POST('/api/v1/midi/refresh');
  if (data) useMidi.getState().set(data);
}

/** Wording for a refused route, in the console's voice. */
export function routeErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 404) return 'that instrument is no longer in the rack';
  return detail ?? 'the route was refused';
}

/**
 * Apply one change. Resolves to an error sentence the caller shows inline,
 * or null on success.
 */
export async function routeMidi(body: MidiRouteRequest): Promise<string | null> {
  useMidi.getState().setPending(true);
  try {
    const { data, error, response } = await $api.PUT('/api/v1/midi/routes', { body });
    if (data) {
      useMidi.getState().set(data);
      return null;
    }
    useMidi.getState().setPending(false);
    const detail = (error as { detail?: string } | undefined)?.detail;
    return routeErrorMessage(response.status, detail);
  } catch {
    useMidi.getState().setPending(false);
    return routeErrorMessage('network');
  }
}
