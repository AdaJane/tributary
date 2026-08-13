/**
 * Sessions on the current drive. Scoped to the active destination by the
 * daemon, so changing the drive changes this list — there is no global
 * index and no cross-drive scan.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';
import { sessionErrorMessage } from '../features/setup/session-logic';
import type { SessionSeed } from '../features/setup/session-logic';

type SessionsDto = components['schemas']['SessionsDto'];

export interface Session {
  id: string;
  name: string;
  createdAtUnix: number;
  takeCount: number;
  open: boolean;
}

interface SessionsState {
  sessions: Session[];
  /** The projects root they live under — the active destination. */
  root: string;
  /** False when the drive has gone away. An empty list then means "we
   *  cannot see them", not "there are none" — two different sentences. */
  rootPresent: boolean;
  loaded: boolean;
  error: string | null;
  /** The session an action is in flight for. */
  pending: string | null;
  apply: (dto: SessionsDto) => void;
  beginPending: (id: string) => void;
  fail: (message: string) => void;
}

export const useSessions = create<SessionsState>((set) => ({
  sessions: [],
  root: '',
  rootPresent: true,
  loaded: false,
  error: null,
  pending: null,
  apply: (dto) =>
    set({
      sessions: dto.sessions.map((s) => ({
        id: s.id,
        name: s.name,
        createdAtUnix: s.created_at_unix,
        takeCount: s.take_count,
        open: s.open,
      })),
      root: dto.root,
      rootPresent: dto.root_present,
      loaded: true,
      pending: null,
      error: null,
    }),
  beginPending: (id) => set({ pending: id, error: null }),
  fail: (error) => set({ error, pending: null }),
}));

export async function loadSessions(): Promise<void> {
  const { data } = await $api.GET('/api/v1/sessions');
  if (data) useSessions.getState().apply(data);
}

/** The name of the open session, for the console's tape label. */
export function openSessionName(): string | null {
  return useSessions.getState().sessions.find((s) => s.open)?.name ?? null;
}

async function act(
  id: string,
  run: () => Promise<{ error?: unknown; response: Response }>,
): Promise<string | null> {
  useSessions.getState().beginPending(id);
  try {
    const { error, response } = await run();
    if (!error) {
      await loadSessions();
      return null;
    }
    const detail = (error as { detail?: string } | undefined)?.detail;
    const message = sessionErrorMessage(response.status, detail);
    useSessions.getState().fail(message);
    return message;
  } catch {
    const message = sessionErrorMessage('network');
    useSessions.getState().fail(message);
    return message;
  }
}

/**
 * Create a session and open it. Not optimistic: the new desk arrives on
 * the mixer channel and the new (empty) shelf on the transport channel —
 * predicting either would be guessing at what the daemon builds.
 */
export function createSession(name: string, seed: SessionSeed): Promise<string | null> {
  return act('', () => $api.POST('/api/v1/sessions', { body: { name, seed } }));
}

export function openSession(id: string): Promise<string | null> {
  return act(id, () =>
    $api.POST('/api/v1/sessions/{id}/open', { params: { path: { id } } }),
  );
}

export function renameSession(id: string, name: string): Promise<string | null> {
  return act(id, () => $api.PUT('/api/v1/sessions/{id}', { params: { path: { id } }, body: { name } }));
}

export function deleteSession(id: string): Promise<string | null> {
  return act(id, () => $api.DELETE('/api/v1/sessions/{id}', { params: { path: { id } } }));
}
