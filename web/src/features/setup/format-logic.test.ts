import { describe, expect, it } from 'vitest';

import {
  CONFIRM_WORD,
  DEFAULT_LABEL,
  FILESYSTEMS,
  FILESYSTEM_COPY,
  confirmReady,
  defaultFilesystem,
  formatErrorMessage,
  formatPromise,
  validateLabel,
} from './format-logic';

describe('validateLabel', () => {
  it('accepts the default and ordinary names', () => {
    expect(validateLabel(DEFAULT_LABEL)).toBeNull();
    expect(validateLabel('FIELD_REC-1')).toBeNull();
  });

  /** 11 characters is exFAT's limit; accepting 12 here would mean the UI
   *  greenlights a label the format then rejects. */
  it('stops at exFAT’s 11-character limit', () => {
    expect(validateLabel('12345678901')).toBeNull();
    expect(validateLabel('123456789012')).toBe('at most 11 characters');
  });

  it('refuses empty and non-ASCII names', () => {
    expect(validateLabel('')).toBe('name the drive');
    expect(validateLabel('has space')).toBe('letters, digits, . _ - only');
    expect(validateLabel('a/b')).toBe('letters, digits, . _ - only');
    expect(validateLabel('café')).toBe('letters, digits, . _ - only');
  });
});

describe('confirmReady', () => {
  it('needs the exact word, and a valid label alongside it', () => {
    expect(confirmReady(CONFIRM_WORD, 'TRIBUTARY')).toBe(true);
    expect(confirmReady('erase', 'TRIBUTARY')).toBe(false);
    expect(confirmReady('ERASE ', 'TRIBUTARY')).toBe(false);
    expect(confirmReady('', 'TRIBUTARY')).toBe(false);
    // A drive you cannot name is a drive you cannot format.
    expect(confirmReady(CONFIRM_WORD, '')).toBe(false);
  });

  it('cannot be satisfied by a reflex word', () => {
    for (const guess of ['y', 'yes', 'ok', 'OK', 'sure', 'delete']) {
      expect(confirmReady(guess, 'TRIBUTARY')).toBe(false);
    }
  });
});

describe('formatPromise', () => {
  /** The whole argument for the feature: the user's stick shows 3.5 GB of
   *  partitions on 58 GB of hardware. */
  it('states the full capacity the drive will end up with', () => {
    expect(formatPromise(62_026_416_128, 'TRIBUTARY', 'exfat')).toBe(
      '1 partition · exFAT · TRIBUTARY · 62.0 GB',
    );
  });

  /** The promise names the filesystem actually chosen. Printing "exFAT"
   *  over an ext4 format would be a lie in the one dialog that must not
   *  tell any. */
  it('names the filesystem being laid down', () => {
    expect(formatPromise(2_000_398_934_016, 'STAGE', 'ext4')).toBe(
      '1 partition · ext4 · STAGE · 2.0 TB',
    );
  });
});

describe('defaultFilesystem', () => {
  /** A stick is going somewhere else, so it gets the one every laptop
   *  reads; a fixed disk is staying, so it gets the journalled one. */
  it('follows where the drive lives', () => {
    expect(defaultFilesystem(true)).toBe('exfat');
    expect(defaultFilesystem(false)).toBe('ext4');
  });

  /** Both options are always offered — the default is a starting point,
   *  not a restriction. */
  it('offers both filesystems whatever the default', () => {
    expect(FILESYSTEMS).toContain('exfat');
    expect(FILESYSTEMS).toContain('ext4');
  });

  /** "exFAT" and "ext4" tell a musician nothing; the consequence does. */
  it('explains each choice in words, differently', () => {
    expect(FILESYSTEM_COPY.exfat.detail).not.toBe(FILESYSTEM_COPY.ext4.detail);
    expect(FILESYSTEM_COPY.exfat.detail).toMatch(/Mac|PC/);
  });
});

describe('formatErrorMessage', () => {
  it('maps each refusal to something actionable', () => {
    expect(formatErrorMessage(409, 'stop recording first')).toBe('stop recording first');
    expect(formatErrorMessage(422, '/dev/sda is the disk the appliance runs from')).toBe(
      '/dev/sda is the disk the appliance runs from',
    );
    expect(formatErrorMessage('network')).toBe('daemon unreachable');
  });

  /** A package install ships no privileged helper. That is a permanent
   *  fact about the platform, not a user error. */
  it('explains an installation that cannot format at all', () => {
    expect(formatErrorMessage(501)).toMatch(/not available/);
  });
});
