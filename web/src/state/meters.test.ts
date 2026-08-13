import { beforeEach, describe, expect, it } from 'vitest';

import { FADER_MIN_DB } from '../audio/db';
import { SILENT_READING, readingClip, useMeters } from './meters';

const batch = (entries: [string, number, boolean][]) =>
  new Map(entries.map(([k, peakDb, clip]) => [k, { peakDb, clip }]));

beforeEach(() => useMeters.setState({ byKey: {} }));

describe('useMeters', () => {
  it('applies a batch and latches a clip', () => {
    useMeters.getState().applyBatch(batch([['strip:0', -12, true]]), 1000);
    const reading = useMeters.getState().byKey['strip:0'];
    expect(reading.peakDb).toBe(-12);
    expect(readingClip(reading, 1000)).toBe(true);
  });

  it('clearClip drops the latch but keeps the level', () => {
    useMeters.getState().applyBatch(batch([['strip:0', -12, true]]), 1000);
    useMeters.getState().clearClip('strip:0');
    const reading = useMeters.getState().byKey['strip:0'];
    expect(reading.peakDb).toBe(-12);
    expect(readingClip(reading, 1000)).toBe(false);
  });

  it('clear drops every reading, so nothing is left frozen mid-level', () => {
    // The failure this exists for: with the socket gone, held readings look
    // exactly like live ones — the console's most alarming lie.
    useMeters.getState().applyBatch(
      batch([
        ['strip:0', -6, true],
        ['master', -3, false],
      ]),
      1000,
    );
    useMeters.getState().clear();
    expect(useMeters.getState().byKey).toEqual({});
  });

  it('a cleared key reads as silence, not as missing data', () => {
    useMeters.getState().applyBatch(batch([['strip:0', -6, false]]), 1000);
    useMeters.getState().clear();
    const reading = useMeters.getState().byKey['strip:0'] ?? SILENT_READING;
    expect(reading.peakDb).toBe(FADER_MIN_DB);
    expect(readingClip(reading, 1000)).toBe(false);
  });

  it('clearing an already-empty store keeps the same object', () => {
    const before = useMeters.getState().byKey;
    useMeters.getState().clear();
    expect(useMeters.getState().byKey).toBe(before);
  });
});
