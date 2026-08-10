import { describe, expect, it } from 'vitest';

import {
  destinationStatus,
  formatBytes,
  needsRestart,
  owningPath,
  saveErrorMessage,
} from './settings-logic';

describe('formatBytes', () => {
  it('covers the unit boundaries', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1_000)).toBe('1.0 KB');
    expect(formatBytes(512_000_000)).toBe('512 MB');
    expect(formatBytes(14_200_000_000)).toBe('14.2 GB');
    expect(formatBytes(1_000_000_000_000)).toBe('1.0 TB');
    expect(formatBytes(2_500_000_000_000_000)).toBe('2.5 PB');
  });
});

describe('destinationStatus', () => {
  const drives = [
    { mountPath: '/', writable: true },
    { mountPath: '/run/media/user/STICK', writable: true },
    { mountPath: '/run/media/user/LOCKED', writable: false },
  ];

  it('reports ready on a writable mount, by longest prefix', () => {
    expect(destinationStatus('/run/media/user/STICK/trib', null, drives)).toBe('ready');
    expect(destinationStatus('/home/user/projects', null, drives)).toBe('ready');
  });

  it('reports read-only from the owning mount, not the root', () => {
    expect(destinationStatus('/run/media/user/LOCKED', null, drives)).toBe('read-only');
    expect(destinationStatus('/run/media/user/LOCKED/x', null, drives)).toBe('read-only');
  });

  it('does not treat a sibling mount name as a prefix', () => {
    expect(
      destinationStatus('/run/media/user/LOCKED2', null, [
        { mountPath: '/run/media/user/LOCKED', writable: false },
      ]),
    ).toBe('ready');
  });

  it('reports missing when the daemon fell back from the configured drive', () => {
    expect(destinationStatus('/home/user/projects', '/run/media/user/GONE', drives)).toBe('missing');
  });

  it('stays optimistic with no matching mount', () => {
    expect(destinationStatus('/somewhere/odd', null, [])).toBe('ready');
  });
});

describe('owningPath', () => {
  const paths = ['/home/user/projects', '/run/media/user/STICK', '/run/media/user/STICK/tributary'];

  it('picks the longest containing path, exactly one winner', () => {
    expect(owningPath('/run/media/user/STICK/tributary', paths)).toBe(
      '/run/media/user/STICK/tributary',
    );
    expect(owningPath('/run/media/user/STICK/other', paths)).toBe('/run/media/user/STICK');
    expect(owningPath('/home/user/projects', paths)).toBe('/home/user/projects');
  });

  it('returns null when nothing contains the destination', () => {
    expect(owningPath('/tmp/elsewhere', paths)).toBeNull();
    expect(owningPath('/run/media/user/STICKY', paths)).toBeNull();
  });
});

describe('needsRestart', () => {
  it('compares rates when the server offers no verdict', () => {
    expect(needsRestart(96_000, 48_000)).toBe(true);
    expect(needsRestart(48_000, 48_000)).toBe(false);
  });

  it('lets the server flag win in both directions', () => {
    expect(needsRestart(96_000, 48_000, false)).toBe(false);
    expect(needsRestart(48_000, 48_000, true)).toBe(true);
  });
});

describe('saveErrorMessage', () => {
  it('maps the status table', () => {
    expect(saveErrorMessage(409)).toBe('locked while recording');
    expect(saveErrorMessage(422)).toBe('the daemon refused the setting');
    expect(saveErrorMessage(422, '/mnt/x is not writable')).toBe('/mnt/x is not writable');
    expect(saveErrorMessage('network')).toBe('daemon unreachable');
    expect(saveErrorMessage(500)).toBe('save failed');
  });
});
