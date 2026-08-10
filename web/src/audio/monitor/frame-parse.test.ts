import { describe, expect, it } from 'vitest';

import { parseMonitorFrame } from './frame-parse';

function frame(generation: number, startFrame: number, samples: number[]): ArrayBuffer {
  const buffer = new ArrayBuffer(24 + samples.length * 4);
  const view = new DataView(buffer);
  [0x54, 0x4d, 0x4f, 0x4e].forEach((b, i) => view.setUint8(i, b));
  view.setUint32(4, generation, true);
  view.setBigUint64(8, BigInt(startFrame), true);
  view.setUint32(16, samples.length / 2, true);
  samples.forEach((s, i) => view.setFloat32(24 + i * 4, s, true));
  return buffer;
}

describe('parseMonitorFrame', () => {
  it('decodes the header and views the samples in place', () => {
    const parsed = parseMonitorFrame(frame(7, 96_000, [0.5, -0.25, 0.1, 0.2]));
    expect(parsed?.generation).toBe(7);
    expect(parsed?.startFrame).toBe(96_000);
    expect(parsed?.frameCount).toBe(2);
    expect(Array.from(parsed?.samples ?? [])).toEqual(
      [0.5, -0.25, 0.1, 0.2].map(Math.fround),
    );
  });

  it('rejects bad magic and short bodies', () => {
    expect(parseMonitorFrame(new ArrayBuffer(10))).toBeNull();
    const bad = frame(1, 0, [0, 0]);
    new DataView(bad).setUint8(0, 0x58);
    expect(parseMonitorFrame(bad)).toBeNull();
    expect(parseMonitorFrame(frame(1, 0, [0, 0]).slice(0, 26))).toBeNull();
  });
});
