import { describe, expect, it } from 'vitest';

import { parsePeaks } from './peaks-parse';

function body(tracks: number[][], samplesPerBin = 512): ArrayBuffer {
  const pairBytes = tracks.reduce((n, t) => n + t.length * 2, 0);
  const buffer = new ArrayBuffer(12 + tracks.length * 4 + pairBytes);
  const view = new DataView(buffer);
  [0x54, 0x50, 0x4b, 0x53].forEach((b, i) => view.setUint8(i, b)); // "TPKS"
  view.setUint8(4, 1);
  view.setUint16(6, tracks.length, true);
  view.setUint32(8, samplesPerBin, true);
  let offset = 12;
  for (const track of tracks) {
    view.setUint32(offset, track.length / 2, true);
    offset += 4;
    for (const v of track) {
      view.setInt16(offset, v, true);
      offset += 2;
    }
  }
  return buffer;
}

describe('parsePeaks', () => {
  it('parses tracks with their interleaved pairs', () => {
    const parsed = parsePeaks(body([[-100, 200], [-1, 1, -2, 2]]));
    expect(parsed).not.toBeNull();
    expect(parsed?.samplesPerBin).toBe(512);
    expect(Array.from(parsed?.tracks[0] ?? [])).toEqual([-100, 200]);
    expect(Array.from(parsed?.tracks[1] ?? [])).toEqual([-1, 1, -2, 2]);
  });

  it('rejects bad magic and truncated bodies', () => {
    expect(parsePeaks(new ArrayBuffer(4))).toBeNull();
    const bad = body([[-1, 1]]);
    new DataView(bad).setUint8(0, 0x58);
    expect(parsePeaks(bad)).toBeNull();
    const truncated = body([[-1, 1, -2, 2]]).slice(0, 14);
    expect(parsePeaks(truncated)).toBeNull();
  });

  it('parses an empty take (zero tracks)', () => {
    expect(parsePeaks(body([]))?.tracks).toEqual([]);
  });
});
