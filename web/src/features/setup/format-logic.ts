/**
 * Pure rules for the format dialog. Wiping a drive is irreversible and the
 * console is reachable by anyone on the appliance's open Wi-Fi, so the
 * gate is deliberately heavier than the two-click SURE? used for removing
 * a channel strip: a strip costs nothing to rebuild, someone's recordings
 * do not.
 *
 * The label rule is duplicated in the daemon and the helper. Theirs is
 * authoritative (a 422); this copy exists only to disable the button
 * before a round trip, and both are driven by the same table.
 */

import { formatBytes } from './settings-logic';

/** exFAT's volume-label limit — a filesystem fact, not a style choice. */
export const MAX_LABEL = 11;

export const DEFAULT_LABEL = 'TRIBUTARY';

/** The word the user must type. Not "yes" — it should be impossible to
 *  produce by reflex. */
export const CONFIRM_WORD = 'ERASE';

/** `null` when the label is acceptable, else why it is not. */
export function validateLabel(label: string): string | null {
  if (label.length === 0) return 'name the drive';
  if (label.length > MAX_LABEL) return `at most ${MAX_LABEL} characters`;
  if (!/^[A-Za-z0-9_.-]+$/.test(label)) return 'letters, digits, . _ - only';
  return null;
}

export function confirmReady(typed: string, label: string): boolean {
  return typed === CONFIRM_WORD && validateLabel(label) === null;
}

/**
 * What the drive becomes. Printed next to what it currently holds, so the
 * reclaimed space is legible rather than magic — the case that prompted
 * this feature is a 58 GB stick showing 3.5 GB of partitions.
 */
export function formatPromise(totalBytes: number, label: string): string {
  return `1 partition · exFAT · ${label || '—'} · ${formatBytes(totalBytes)}`;
}

/** Message for a refused format, matching the settings error table. */
export function formatErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 409) return detail ?? 'the drive is busy';
  if (status === 422) return detail ?? 'the daemon refused the drive';
  if (status === 501) return detail ?? 'formatting is not available on this installation';
  return detail ?? 'format failed';
}
