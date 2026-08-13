/**
 * Pure logic for the Setup view: byte formatting, destination status, the
 * restart gate, and the save-error message table. No DTO imports — the
 * store maps wire shapes; this file stays structural.
 */

export type DriveStatus = 'ready' | 'read-only' | 'missing';

export interface DriveLike {
  /** `null` when the drive is present but nothing mounted it. */
  mountPath: string | null;
  /** The daemon's verdict. Only `ready` means "recording will work". */
  state: string;
}

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'] as const;

/** Decimal units, three significant figures: "512 MB", "14.2 GB". */
export function formatBytes(bytes: number): string {
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < BYTE_UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const text = unit === 0 || value >= 100 ? Math.round(value).toString() : value.toFixed(1);
  return `${text} ${BYTE_UNITS[unit]}`;
}

/**
 * The REC PATH lamp. The daemon's configured-vs-active divergence is the
 * authoritative "drive missing" signal (it fell back at boot); otherwise
 * the longest matching mount decides writability. An unmatched path stays
 * optimistic — the daemon 422s on save if it's actually bad.
 */
export function destinationStatus(
  destination: string,
  configuredDestination: string | null,
  drives: readonly DriveLike[],
): DriveStatus {
  if (configuredDestination !== null && configuredDestination !== destination) {
    return 'missing';
  }
  const mount = drives
    .filter((d) => d.mountPath !== null)
    .filter(
      (d) => destination === d.mountPath || destination.startsWith(withSlash(d.mountPath as string)),
    )
    .reduce<DriveLike | null>(
      (best, d) =>
        best === null || (d.mountPath as string).length > (best.mountPath as string).length
          ? d
          : best,
      null,
    );
  if (mount === null) return 'ready';
  return mount.state === 'ready' ? 'ready' : 'read-only';
}

function withSlash(mount: string): string {
  return mount.endsWith('/') ? mount : `${mount}/`;
}

/**
 * Which tile owns the current destination: the longest path that equals or
 * contains it. Exactly one tile lights, even when mounts nest.
 */
export function owningPath(destination: string, paths: readonly string[]): string | null {
  return paths
    .filter((p) => destination === p || destination.startsWith(withSlash(p)))
    .reduce<string | null>((best, p) => (best === null || p.length > best.length ? p : best), null);
}

/** The daemon's verdict wins when present; else compare the rates. */
export function needsRestart(
  configuredHz: number,
  activeHz: number,
  serverFlag?: boolean,
): boolean {
  return serverFlag ?? configuredHz !== activeHz;
}

/** Inline error line per panel: short, print-style, no codes. */
export function saveErrorMessage(status: number | 'network', detail?: string): string {
  if (status === 'network') return 'daemon unreachable';
  if (status === 409) return 'locked while recording';
  if (status === 422) return detail ?? 'the daemon refused the setting';
  return detail ?? 'save failed';
}
