import { describe, expect, it } from 'vitest';

import type { SoundfontInfo } from '../../state/instruments';
import {
  defaultSoundfont,
  deleteBlocked,
  emptyState,
  formatBytes,
  groupSoundfonts,
  progressLine,
  ramNotice,
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

describe('the shipped library', () => {
  const font = (over: Partial<SoundfontInfo>): SoundfontInfo => ({
    id: 'x.sf2',
    path: '/usr/share/tributary/soundfonts/x.sf2',
    bytes: 1_000,
    origin: 'built_in',
    ...over,
  });

  /** On a fresh appliance the built-ins are the only sounds there are, so
   *  an empty "Internal" heading above them would read as an empty box. */
  it('lists built-in sounds above uploads and drives', () => {
    const groups = groupSoundfonts([
      font({ id: 'mine.sf2', origin: 'internal' }),
      font({ id: 'GeneralUser-GS.sf2' }),
      font({ id: 'theirs.sf2', origin: 'removable', volume: 'STICK' }),
    ]);
    expect(groups.map((g) => g.title)).toEqual(['Built in', 'Internal', 'STICK']);
    expect(groups[0].builtIn).toBe(true);
    expect(groups[0].files[0].id).toBe('GeneralUser-GS.sf2');
  });

  /** A build that bundled nothing must not show a heading promising some. */
  it('shows no built-in heading when nothing was bundled', () => {
    const groups = groupSoundfonts([font({ id: 'mine.sf2', origin: 'internal' })]);
    expect(groups.map((g) => g.title)).toEqual(['Internal']);
  });

  /** They belong to the installation, and there is no way to put one back
   *  — an upload cannot take a built-in's name. */
  it('refuses to delete a built-in, with a reason rather than a hidden button', () => {
    const blocked = deleteBlocked(font({}), [], false);
    expect(blocked).toBe('built in — part of this installation');
  });

  /** Size is memory here, so a large bank says so in words. */
  it('warns in plain language about a font large enough to matter', () => {
    expect(ramNotice(1_000)).toBeNull();
    expect(ramNotice(32_319_396)).toBeNull();
    expect(ramNotice(215_614_036)).toMatch(/memory/);
    expect(ramNotice(215_614_036)).toMatch(/206 MB/);
  });

  /** The smallest, because a new instrument silently costing 206 MB on a
   *  2 GB Pi is a bad way to meet the feature. */
  it('starts a new instrument on the smallest built-in', () => {
    expect(
      defaultSoundfont([
        font({ id: 'MuseScore_General.sf2', bytes: 215_614_036 }),
        font({ id: 'GeneralUser-GS.sf2', bytes: 32_319_396 }),
        font({ id: 'FluidR3_GM.sf2', bytes: 148_398_306 }),
      ]),
    ).toBe('GeneralUser-GS.sf2');
  });

  it('falls back to an upload, then to no voice at all', () => {
    expect(defaultSoundfont([font({ id: 'mine.sf2', origin: 'internal' })])).toBe('mine.sf2');
    // A font on someone's stick is not a default: it leaves with them.
    expect(
      defaultSoundfont([font({ id: 'theirs.sf2', origin: 'removable', volume: 'S' })]),
    ).toBeUndefined();
    expect(defaultSoundfont([])).toBeUndefined();
  });
});
