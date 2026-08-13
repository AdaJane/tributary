/**
 * Pure rules for the session panel: naming, ordering, and what each
 * create-seed costs you. No DTO imports — the store maps wire shapes and
 * this file stays structural, the same split `settings-logic.ts` uses.
 */

/** Mirrors the daemon's limit. The name is slugged into a directory
 *  component, and the daemon's answer is authoritative — this copy only
 *  disables the button before a round trip. */
export const MAX_SESSION_NAME = 64;

export type SessionSeed = 'template' | 'mapping' | 'console';

export interface SessionLike {
  id: string;
  name: string;
  createdAtUnix: number;
  takeCount: number;
  open: boolean;
}

/** `null` when the name is acceptable, else why it is not. */
export function validateSessionName(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed.length === 0) return 'name the session';
  if (trimmed.length > MAX_SESSION_NAME) return `at most ${MAX_SESSION_NAME} characters`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f]/.test(trimmed)) return 'one line, no control characters';
  return null;
}

/**
 * Why this session cannot be deleted, or `null` if it can.
 *
 * The open one is refused daemon-side too: one gesture would otherwise
 * make two state changes, the second irreversible — and if it were the
 * only session, "delete" would have to create one.
 */
export function deleteBlocked(session: SessionLike, recording: boolean): string | null {
  if (recording) return 'stop recording first';
  if (session.open) return 'open another session first';
  return null;
}

/** Why this session cannot be opened, or `null` if it can. */
export function openBlocked(session: SessionLike, recording: boolean): string | null {
  if (recording) return 'stop recording first';
  if (session.open) return 'already open';
  return null;
}

/** "4 takes" / "1 take" / "empty". */
export function takeCountLabel(count: number): string {
  if (count === 0) return 'empty';
  return count === 1 ? '1 take' : `${count} takes`;
}

export const SEED_LABELS: Record<SessionSeed, string> = {
  template: 'Clean desk',
  mapping: 'Same channels',
  console: 'Copy this desk',
};

/**
 * What the chosen seed will do to the desk you are looking at, in one
 * line. The label says what you get; this says what you lose — and it is
 * built here rather than hand-written per branch so the strip count and
 * the wording are testable.
 */
export function seedConsequence(seed: SessionSeed, stripCount: number, leaving: string): string {
  const strips = stripCount === 1 ? '1 channel' : `${stripCount} channels`;
  switch (seed) {
    case 'console':
      return 'The desk carries over exactly as it is — nothing changes here.';
    case 'mapping':
      return `${strips} keep their inputs and arming; EQ, sends and levels reset. “${leaving}” keeps its own copy.`;
    case 'template':
      return `Back to one channel on input 1. “${leaving}” keeps your current desk.`;
  }
}

/** Only a seed that changes the desk is worth confirming — a dialog that
 *  fires when nothing will happen is the kind that gets tapped through. */
export function seedNeedsConfirm(seed: SessionSeed): boolean {
  return seed !== 'console';
}

/** Newest first, and never trusting the wire to have sorted. */
export function sortedSessions<T extends { createdAtUnix: number; id: string }>(
  sessions: readonly T[],
): T[] {
  return [...sessions].sort(
    (a, b) => b.createdAtUnix - a.createdAtUnix || b.id.localeCompare(a.id),
  );
}

/** Inline error line for a session action, matching `saveErrorMessage`. */
export function sessionErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 404) return 'that session is no longer there';
  if (status === 409) return detail ?? 'the daemon refused — something is in the way';
  if (status === 422) return detail ?? 'the daemon refused that name';
  return detail ?? 'that did not work';
}
