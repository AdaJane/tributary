/**
 * The open session's takes. Mirrors the daemon: seeded by REST, kept live
 * by the `takes_changed` push, and never optimistic — a selection is
 * confirmed by the transport push, not predicted, the same rule
 * `setCardProfile` and `formatDrive` already follow.
 *
 * Holding the list here is also what fixes `usePeaks`' over-fetch: it used
 * to GET the whole take list on every take change and throw away all but
 * one row.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';
import { deleteErrorMessage, selectErrorMessage } from '../features/tracks/take-errors';

type TakeDto = components['schemas']['TakeDto'];

export interface TakeTrack {
  file: string;
  channels: number;
  stripId: number | null;
  /** Pre-derived: the writer counts dropped samples, the UI shows a chip. */
  damaged: boolean;
  /** This track's instrument also left a `.mid` beside the audio. */
  midi: boolean;
}

export interface Take {
  take: number;
  startedAtUnix: number;
  sampleRate: number;
  damaged: boolean;
  durationSecs: number;
  tracks: TakeTrack[];
}

export type TakesStatus = 'idle' | 'loading' | 'ready' | 'error';

interface TakesState {
  takes: Take[];
  status: TakesStatus;
  error: string | null;
  /** The take an action is in flight for, so a row can show it without
   *  pretending the action already succeeded. */
  pending: number | null;
  setTakes: (takes: Take[]) => void;
  beginPending: (take: number) => void;
  fail: (message: string) => void;
  clearError: () => void;
}

export const useTakes = create<TakesState>((set) => ({
  takes: [],
  status: 'idle',
  error: null,
  pending: null,
  setTakes: (takes) => set({ takes, status: 'ready', pending: null, error: null }),
  beginPending: (take) => set({ pending: take, error: null }),
  fail: (error) => set({ error, pending: null }),
  clearError: () => set({ error: null }),
}));

const fromDtos = (dtos: readonly TakeDto[]): Take[] =>
  dtos.map((t) => ({
    take: t.take,
    startedAtUnix: t.started_at_unix,
    sampleRate: t.sample_rate,
    damaged: t.damaged,
    durationSecs: t.duration_secs,
    tracks: t.tracks.map((track) => ({
      file: track.file,
      channels: track.channels,
      stripId: track.strip_id ?? null,
      damaged: track.dropped_samples > 0,
      // A sidecar belongs to an instrument, and an instrument can feed
      // more than one strip — so every lane fed by one carries the chip.
      midi: (t.midi_tracks ?? []).length > 0 && track.strip_id !== null,
    })),
  }));

/** REST seed. The push keeps it current from there. */
export async function loadTakes(): Promise<void> {
  const { data } = await $api.GET('/api/v1/takes');
  if (data) useTakes.getState().setTakes(fromDtos(data));
}

/** A pushed list from `takes_changed` — same mapping as the GET, so a
 *  push and a poll can never disagree. */
export function applyTakes(dtos: readonly TakeDto[]): void {
  useTakes.getState().setTakes(fromDtos(dtos));
}

/** Point the transport at a take. `null` on success, else a sentence. */
export async function selectTake(take: number): Promise<string | null> {
  useTakes.getState().beginPending(take);
  try {
    const { error, response } = await $api.PUT('/api/v1/transport/take', { body: { take } });
    if (!error) {
      useTakes.setState({ pending: null });
      return null;
    }
    const detail = (error as { detail?: string } | undefined)?.detail;
    const message = selectErrorMessage(response.status, detail);
    useTakes.getState().fail(message);
    return message;
  } catch {
    const message = selectErrorMessage('network');
    useTakes.getState().fail(message);
    return message;
  }
}

/** Permanently remove a take. `null` on success, else a sentence. */
export async function removeTake(take: number): Promise<string | null> {
  useTakes.getState().beginPending(take);
  try {
    const { data, error, response } = await $api.DELETE('/api/v1/takes/{take}', {
      params: { path: { take } },
    });
    if (data) {
      // The daemon hands back the remaining list, so the browser is
      // correct without waiting for the push to arrive.
      useTakes.getState().setTakes(fromDtos(data));
      return null;
    }
    const detail = (error as { detail?: string } | undefined)?.detail;
    const message = deleteErrorMessage(response.status, detail);
    useTakes.getState().fail(message);
    return message;
  } catch {
    const message = deleteErrorMessage('network');
    useTakes.getState().fail(message);
    return message;
  }
}
