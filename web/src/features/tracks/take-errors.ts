/**
 * Sentences for the take browser's refusals, in the shape
 * `settings-logic.saveErrorMessage` established.
 *
 * These exist because the Tracks view used to swallow every failure —
 * `.catch(() => undefined)`, and a non-ok fetch resolving to `null` — so a
 * broken take rendered as an empty room with no explanation. A browser
 * makes picking a broken take a deliberate act, and silence is the wrong
 * answer to a deliberate act.
 */

export function selectErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 404) return 'that take is no longer there';
  if (status === 409) return detail ?? 'stop recording first';
  return detail ?? 'could not switch to that take';
}

export function deleteErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 404) return 'that take is already gone';
  if (status === 409) return detail ?? 'stop first — that take is in use';
  return detail ?? 'could not delete that take';
}

export function peaksErrorMessage(status: number | 'network' | 'malformed'): string {
  if (status === 'network') return 'daemon unreachable — the waveform could not load';
  if (status === 'malformed') return 'that take’s waveform data is unreadable';
  if (status === 404) return 'that take is no longer on the drive';
  return 'the waveform could not load';
}
