import { describe, expect, it } from 'vitest';

import { EMPTY_STATE_COPY, emptyState, groupDrives } from './drives-logic';
import type { DriveLike } from './drives-logic';

const DEFAULT_DEST = '/home/tributary/projects';

function drive(over: Partial<DriveLike> = {}): DriveLike {
  return {
    mountPath: '/media/FIELD',
    device: '/dev/sdb1',
    label: 'FIELD',
    filesystem: 'exfat',
    totalBytes: 58_000_000_000,
    freeBytes: 57_000_000_000,
    removable: true,
    transport: 'usb',
    state: 'ready',
    reason: null,
    disk: '/dev/sdb',
    ...over,
  };
}

/**
 * The exact drive that prompted this work, as the daemon now reports it:
 * a 57.8 GB stick carrying a flashed OS image, so both of its volumes are
 * unusable and ~54 GB is unallocated.
 */
const FLASHED_IMAGE: DriveLike[] = [
  drive({
    device: '/dev/sda1',
    disk: '/dev/sda',
    label: 'bootfs',
    filesystem: 'vfat',
    mountPath: '/media/bootfs',
    totalBytes: 536_870_912,
    freeBytes: 450_450_944,
    state: 'too_small',
    reason: 'too small to record onto',
  }),
  drive({
    device: '/dev/sda2',
    disk: '/dev/sda',
    label: 'rootfs',
    filesystem: 'ext4',
    mountPath: '/media/rootfs',
    totalBytes: 3_238_002_688,
    freeBytes: 1_187_540_992,
    state: 'not_writable',
    reason: 'mounted, but the recorder cannot write to it',
  }),
];

describe('groupDrives', () => {
  it('always offers Internal first', () => {
    const groups = groupDrives(DEFAULT_DEST, []);
    expect(groups[0].key).toBe('internal');
    expect(groups[0].tiles[0].path).toBe(DEFAULT_DEST);
  });

  it('groups a flashed-image stick under one drive, hiding nothing', () => {
    const groups = groupDrives(DEFAULT_DEST, FLASHED_IMAGE);
    const stick = groups.find((g) => g.key === '/dev/sda');
    expect(stick).toBeDefined();
    // Both partitions are still on screen — MASTER.md forbids folding.
    expect(stick?.tiles).toHaveLength(2);
    expect(stick?.tiles.map((t) => t.label)).toEqual(['rootfs', 'bootfs']);
  });

  it('headlines a drive that has no usable volume, and offers no selection', () => {
    const stick = groupDrives(DEFAULT_DEST, FLASHED_IMAGE).find((g) => g.key === '/dev/sda');
    expect(stick?.headline).toBe('no volume on this drive can be recorded to');
    expect(stick?.tiles.every((t) => !t.selectable)).toBe(true);
    expect(stick?.tiles.every((t) => t.path === null)).toBe(true);
  });

  it('carries the daemon reason onto every unusable tile', () => {
    const stick = groupDrives(DEFAULT_DEST, FLASHED_IMAGE).find((g) => g.key === '/dev/sda');
    expect(stick?.tiles.map((t) => t.reason)).toEqual([
      'mounted, but the recorder cannot write to it',
      'too small to record onto',
    ]);
  });

  it('prints capacity and filesystem for an unusable volume, not free space', () => {
    const stick = groupDrives(DEFAULT_DEST, FLASHED_IMAGE).find((g) => g.key === '/dev/sda');
    // "450 MB free" would imply you could record 450 MB onto it.
    expect(stick?.tiles[1].detail).toBe('537 MB · vfat');
  });

  it('prints free space for a ready volume', () => {
    const groups = groupDrives(DEFAULT_DEST, [drive()]);
    expect(groups[1].tiles[0].detail).toBe('57.0 GB free');
    expect(groups[1].tiles[0].path).toBe('/media/FIELD/tributary');
  });

  it('describes an unmounted drive as its capacity, with no path', () => {
    const groups = groupDrives(
      DEFAULT_DEST,
      [drive({ mountPath: null, freeBytes: null, state: 'not_mounted', reason: 'connected, but nothing mounted it' })],
    );
    const tile = groups[1].tiles[0];
    expect(tile.detail).toBe('58.0 GB · exfat');
    expect(tile.selectable).toBe(false);
    expect(tile.reason).toBe('connected, but nothing mounted it');
  });

  it('never offers the system disk as a destination', () => {
    const groups = groupDrives(DEFAULT_DEST, [
      drive({ disk: '/dev/mmcblk0', device: '/dev/mmcblk0p2', state: 'system', removable: false }),
    ]);
    expect(groups).toHaveLength(1);
    expect(groups[0].key).toBe('internal');
  });

  it('ranks a usable drive above a junk-only one', () => {
    const groups = groupDrives(DEFAULT_DEST, [...FLASHED_IMAGE, drive()]);
    expect(groups.map((g) => g.key)).toEqual(['internal', '/dev/sdb', '/dev/sda']);
  });

  it('marks an unformatted drive as such rather than blank', () => {
    const groups = groupDrives(DEFAULT_DEST, [
      drive({ filesystem: null, state: 'unknown_filesystem', mountPath: null, freeBytes: null }),
    ]);
    expect(groups[1].tiles[0].detail).toBe('58.0 GB · unformatted');
  });
});

describe('format guardrails', () => {
  const opts = { canFormat: true, recording: false };

  it('offers Format on a removable whole drive', () => {
    const groups = groupDrives(DEFAULT_DEST, FLASHED_IMAGE, opts);
    expect(groups.find((g) => g.key === '/dev/sda')?.formatBlocked).toBeNull();
  });

  /** The reclaim case: the group reports the drive's full hardware size,
   *  not the 3.5 GB its partitions add up to. */
  it('carries the whole drive capacity, not the partitioned total', () => {
    const stick = groupDrives(DEFAULT_DEST, FLASHED_IMAGE, opts).find((g) => g.key === '/dev/sda');
    expect(stick?.totalBytes).toBe(536_870_912 + 3_238_002_688);
  });

  it('blocks with a reason rather than hiding the control', () => {
    const blocked = (o: Parameters<typeof groupDrives>[2]) =>
      groupDrives(DEFAULT_DEST, FLASHED_IMAGE, o).find((g) => g.key === '/dev/sda')?.formatBlocked;
    expect(blocked({ canFormat: false })).toBe('not available on this installation');
    expect(blocked({ canFormat: true, recording: true })).toBe('stop recording first');
  });

  it('never offers Format on internal storage', () => {
    const groups = groupDrives(DEFAULT_DEST, [drive()], opts);
    expect(groups[0].key).toBe('internal');
    expect(groups[0].formatBlocked).toBe('built-in storage is never formatted');
  });

  /**
   * The blocker this feature exists to remove. This case used to assert
   * `'only removable drives can be formatted'` for exactly this drive:
   * an NVMe SSD is not removable, so the console refused to prepare the
   * best medium the recorder can write to. What may not be wiped is the
   * disk the system booted from, which the daemon reports as
   * `state: 'system'` and which never reaches this function.
   */
  it('offers Format on a fixed NVMe drive', () => {
    const groups = groupDrives(
      DEFAULT_DEST,
      [
        drive({
          removable: false,
          transport: 'nvme',
          device: '/dev/nvme0n1p1',
          disk: '/dev/nvme0n1',
          label: 'STAGE',
        }),
      ],
      opts,
    );
    const nvme = groups.find((g) => g.key === '/dev/nvme0n1');
    expect(nvme?.formatBlocked).toBeNull();
  });

  /** The boot disk is filtered out upstream, so it can never acquire a
   *  Format button by any route through this function. */
  it('drops the system disk entirely rather than blocking it', () => {
    const groups = groupDrives(
      DEFAULT_DEST,
      [drive({ state: 'system', device: '/dev/mmcblk0p2', disk: '/dev/mmcblk0' })],
      opts,
    );
    expect(groups.map((g) => g.key)).toEqual(['internal']);
  });

  /** Identity comes from a device node. A volume the daemon could not
   *  name one for is grouped by label, and there is nothing to format. */
  it('refuses a target that is not a whole-disk node', () => {
    const groups = groupDrives(
      DEFAULT_DEST,
      [drive({ disk: null, device: null, label: 'MYSTERY' })],
      opts,
    );
    expect(groups[1].key).toBe('MYSTERY');
    expect(groups[1].formatBlocked).toBe('not a whole drive');
  });
});

describe('transport', () => {
  /** The word beside the size and the tile's icon both follow how the
   *  drive is attached — description, never permission. */
  it('prints and pictures each transport distinctly', () => {
    const of = (over: Partial<DriveLike>) =>
      groupDrives(DEFAULT_DEST, [drive(over)], {}).find((g) => g.key !== 'internal');

    expect(of({ transport: 'usb' })?.detail).toMatch(/· USB$/);
    expect(of({ transport: 'nvme' })?.detail).toMatch(/· NVMe$/);
    expect(of({ transport: 'sd' })?.detail).toMatch(/· SD$/);

    expect(of({ transport: 'usb' })?.tiles[0].icon).toBe('usb');
    expect(of({ transport: 'nvme' })?.tiles[0].icon).toBe('nvme');
    expect(of({ transport: 'sd' })?.tiles[0].icon).toBe('sd');
  });

  /** A drive lsblk has no word for still prints its size cleanly, with no
   *  dangling separator. */
  it('prints no badge at all for an unknown transport', () => {
    const group = groupDrives(DEFAULT_DEST, [drive({ transport: 'other' })], {}).find(
      (g) => g.key !== 'internal',
    );
    expect(group?.detail).not.toMatch(/·/);
    expect(group?.tiles[0].icon).toBe('drive');
  });
});

describe('emptyState', () => {
  it('distinguishes no drive from a drive that cannot be used', () => {
    expect(emptyState(groupDrives(DEFAULT_DEST, []))).toBe('no-drive');
    expect(emptyState(groupDrives(DEFAULT_DEST, FLASHED_IMAGE))).toBe('unusable-only');
    expect(emptyState(groupDrives(DEFAULT_DEST, [drive()]))).toBeNull();
  });

  it('gives the two states different sentences', () => {
    expect(EMPTY_STATE_COPY['no-drive']).not.toBe(EMPTY_STATE_COPY['unusable-only']);
    // The bug this panel exists to not have: a connected drive reading as
    // an empty port.
    expect(EMPTY_STATE_COPY['unusable-only']).toMatch(/connected/);
    // And it no longer sends anyone to the USB port specifically — an
    // NVMe drive is not plugged into one.
    expect(EMPTY_STATE_COPY['no-drive']).not.toMatch(/USB/);
  });
});
