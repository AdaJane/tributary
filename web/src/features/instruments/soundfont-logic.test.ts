import { describe, expect, it } from 'vitest';

import type { SoundfontInfo } from '../../state/instruments';
import {
  deleteBlocked,
  emptyState,
  formatBytes,
  groupSoundfonts,
  progressLine,
  uploadErrorMessage,
  validateUpload,
} from './soundfont-logic';

function file(over: Partial<SoundfontInfo> = {}): SoundfontInfo {
  return {
    id: 'piano.sf2',
    path: '/var/lib/tributary/soundfonts/piano.sf2',
    bytes: 4 * 1024 * 1024,
    origin: 'internal',
    ...over,
  };
}

describe('grouping', () => {
  it('puts the internal library first and one group per drive', () => {
    const groups = groupSoundfonts([
      file({ id: 'stick.sf2', origin: 'removable', volume: 'STICK' }),
      file(),
    ]);
    expect(groups.map((g) => g.title)).toEqual(['Internal', 'STICK']);
    expect(groups[0].files).toHaveLength(1);
    expect(groups[1].removable).toBe(true);
  });
});

describe('delete gate', () => {
  it('refuses to delete somebody else’s file off their stick', () => {
    const blocked = deleteBlocked(
      file({ origin: 'removable', volume: 'STICK' }),
      [],
      false,
    );
    expect(blocked).toContain('remove it there');
  });

  it('refuses while an instrument is holding it, and names them', () => {
    expect(deleteBlocked(file(), ['Rhodes', 'Pad'], false)).toBe(
      'in use by Rhodes, Pad',
    );
  });

  it('refuses while the tape rolls', () => {
    expect(deleteBlocked(file(), [], true)).toBe('stop recording first');
  });

  it('allows an unused internal file', () => {
    expect(deleteBlocked(file(), [], false)).toBeNull();
  });
});

describe('empty states', () => {
  it('are two different sentences, never merged', () => {
    // A box with nothing loaded and a drive holding no soundfonts have
    // different next steps.
    expect(emptyState([], false)).toContain('upload one below');
    expect(emptyState([], true)).toContain('holds no .sf2 files');
    expect(emptyState([file()], true)).toBe('');
  });
});

describe('upload gate', () => {
  const MAX = 64 * 1024 * 1024;

  it('refuses a file that is not a soundfont', () => {
    expect(validateUpload('holiday.mp4', 10, MAX)).toBe('that is not a .sf2 file');
  });

  it('accepts a windows-cased extension', () => {
    expect(validateUpload('PIANO.SF2', 10, MAX)).toBeNull();
  });

  it('puts both numbers in the too-big sentence', () => {
    // "too big" alone leaves you guessing how much smaller is enough.
    const refusal = validateUpload('big.sf2', 300 * 1024 * 1024, MAX);
    expect(refusal).toContain('300 MB');
    expect(refusal).toContain('64 MB');
  });

  it('refuses an empty file rather than storing nothing', () => {
    expect(validateUpload('empty.sf2', 0, MAX)).toBe('that file is empty');
  });
});

describe('progress', () => {
  it('never sits at 100 % — it changes phase instead', () => {
    // Bytes handed to the socket are not bytes committed: the daemon still
    // has to write and parse them, and a bar frozen at full is the moment
    // people decide the box has hung.
    expect(progressLine(50, 100)).toContain('Uploading…');
    expect(progressLine(100, 100)).toBe('Checking the file…');
  });
});

describe('error dialect', () => {
  it('has its own sentence for every refusal worth acting on', () => {
    expect(uploadErrorMessage(0)).toContain('connection dropped');
    expect(uploadErrorMessage(413)).toContain('too big');
    expect(uploadErrorMessage(422)).toContain('SoundFont');
    expect(uploadErrorMessage(409)).toBe('locked while recording');
    expect(uploadErrorMessage(422, 'that file is truncated')).toBe(
      'that file is truncated',
    );
  });
});

describe('byte formatting', () => {
  it('reads the way a file manager does', () => {
    expect(formatBytes(4 * 1024 * 1024)).toBe('4 MB');
    expect(formatBytes(1536)).toBe('2 KB');
    expect(formatBytes(2 * 1024 ** 3)).toBe('2.0 GB');
  });
});
