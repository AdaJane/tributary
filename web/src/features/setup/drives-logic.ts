/**
 * Turning the daemon's flat drive list into what the Destination panel
 * draws. Pure — the store maps wire shapes, this file only decides shape
 * and order, the same split `groupPatchbay` uses for input devices.
 *
 * The governing rule comes from MASTER.md: "nothing here folds — drive
 * status must never be hidden." So an unusable volume is never dropped and
 * never collapsed behind a disclosure; it is grouped under the drive it
 * came from, de-emphasised, and given a reason. A drive that is plugged in
 * and renders as nothing is indistinguishable from an empty port, which is
 * the bug this whole panel exists to not have.
 */

import { formatBytes } from './settings-logic';

/** Where a drive tile points recording: a tidy subdir, not the drive root. */
export const DRIVE_SUBDIR = 'tributary';

export interface DriveLike {
  mountPath: string | null;
  device: string | null;
  label: string;
  filesystem: string | null;
  totalBytes: number;
  freeBytes: number | null;
  removable: boolean;
  state: string;
  reason: string | null;
  disk: string | null;
}

export interface DriveTile {
  /** Where selecting this tile would record. `null` = not selectable. */
  path: string | null;
  label: string;
  detail: string;
  reason: string | null;
  state: string;
  icon: 'internal' | 'usb' | 'drive';
  selectable: boolean;
}

export interface DriveGroup {
  /** The whole-disk device node — also the format target. */
  key: string;
  title: string;
  detail: string;
  totalBytes: number;
  tiles: DriveTile[];
  /** Set when the group holds no usable volume — the headline case. */
  headline: string | null;
  removable: boolean;
  /** `null` when this drive may be formatted, else why it may not. Never
   *  hidden: a control that vanishes teaches nothing. */
  formatBlocked: string | null;
}

/**
 * A mounted, ready volume reports free space; anything else reports what
 * is actually known about it. Free space on a volume you cannot write to
 * is a meaningless number, so it is not the thing to print.
 */
function detailFor(drive: DriveLike): string {
  const fs = drive.filesystem ?? 'unformatted';
  if (drive.state === 'ready' && drive.freeBytes !== null) {
    return `${formatBytes(drive.freeBytes)} free`;
  }
  return `${formatBytes(drive.totalBytes)} · ${fs}`;
}

function tileFor(drive: DriveLike): DriveTile {
  const usable = drive.state === 'ready' && drive.mountPath !== null;
  return {
    path: usable ? `${drive.mountPath}/${DRIVE_SUBDIR}` : null,
    label: drive.label,
    detail: detailFor(drive),
    reason: drive.reason,
    state: drive.state,
    icon: drive.removable ? 'usb' : 'drive',
    selectable: usable,
  };
}

/**
 * Whether this drive may be wiped, and if not, why. A blocked Format
 * control renders disabled with the reason beside it — never hidden, and
 * never failing on tap, which is the same rule the destination tiles
 * follow.
 */
function formatBlockedReason(
  removable: boolean,
  key: string,
  options: { canFormat?: boolean; recording?: boolean },
): string | null {
  if (options.canFormat === false) return 'not available on this installation';
  if (options.recording) return 'stop recording first';
  if (!removable) return 'only removable drives can be formatted';
  // The daemon and the helper both refuse a non-whole-disk target; this
  // only keeps the button from offering something they would reject.
  if (!/^\/dev\/[a-z]+$/.test(key)) return 'not a whole drive';
  return null;
}

/**
 * Group by whole disk so a flashed OS image reads as one drive that needs
 * preparing, rather than as two mystery partitions sitting as equal peers
 * to a real 58 GB stick. Everything stays on screen.
 */
export function groupDrives(
  defaultDestination: string,
  drives: readonly DriveLike[],
  options: { canFormat?: boolean; recording?: boolean } = {},
): DriveGroup[] {
  const internal: DriveGroup = {
    key: 'internal',
    title: 'Internal',
    detail: 'built-in storage',
    totalBytes: 0,
    removable: false,
    headline: null,
    formatBlocked: 'built-in storage is never formatted',
    tiles: [
      {
        path: defaultDestination,
        label: 'Internal',
        detail: 'default',
        reason: null,
        state: 'ready',
        icon: 'internal',
        selectable: true,
      },
    ],
  };

  const byDisk = new Map<string, DriveLike[]>();
  for (const drive of drives) {
    // The system disk is infrastructure, not a destination the user picks.
    if (drive.state === 'system') continue;
    const key = drive.disk ?? drive.device ?? drive.label;
    const bucket = byDisk.get(key);
    if (bucket) bucket.push(drive);
    else byDisk.set(key, [drive]);
  }

  const groups = [...byDisk.entries()].map<DriveGroup>(([key, members]) => {
    const capacity = members.reduce((sum, d) => sum + d.totalBytes, 0);
    const removable = members.some((d) => d.removable);
    const usable = members.filter((d) => d.state === 'ready');
    const tiles = [...members]
      .sort(
        (a, b) =>
          Number(b.state === 'ready') - Number(a.state === 'ready') ||
          b.totalBytes - a.totalBytes,
      )
      .map(tileFor);
    return {
      key,
      title: members[0].label,
      detail: `${formatBytes(capacity)}${removable ? ' · USB' : ''}`,
      totalBytes: capacity,
      tiles,
      removable,
      headline:
        usable.length === 0 ? 'no volume on this drive can be recorded to' : null,
      formatBlocked: formatBlockedReason(removable, key, options),
    };
  });

  // Removable first, then by usable capacity: a drive with a ready 58 GB
  // volume outranks one holding only junk, whatever their raw sizes.
  groups.sort(
    (a, b) =>
      Number(b.removable) - Number(a.removable) ||
      Number(a.headline !== null) - Number(b.headline !== null) ||
      a.key.localeCompare(b.key),
  );
  return [internal, ...groups];
}

/**
 * What the panel says when there is nothing to pick. These are genuinely
 * different situations and must not share a sentence — "no drive" sends
 * you to the USB port, "unusable" sends you to Format.
 */
export function emptyState(groups: readonly DriveGroup[]): 'no-drive' | 'unusable-only' | null {
  const external = groups.filter((g) => g.key !== 'internal');
  if (external.length === 0) return 'no-drive';
  if (external.every((g) => g.headline !== null)) return 'unusable-only';
  return null;
}

export const EMPTY_STATE_COPY: Record<'no-drive' | 'unusable-only', string> = {
  'no-drive': 'No USB drive connected — plug one in and it appears here on its own.',
  'unusable-only': 'A drive is connected, but nothing on it can be recorded to.',
};
