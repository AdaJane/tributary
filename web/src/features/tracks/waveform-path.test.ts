import { describe, expect, it } from 'vitest';

import { waveformColumns } from './waveform-path';

describe('waveformColumns', () => {
  it('aggregates min and max across the bins a column covers', () => {
    // 4 bins; each column spans 2 bins (fpp = 2 * samplesPerBin).
    const pairs = Int16Array.from([-100, 100, -32767, 200, -50, 32767, 0, 0]);
    const columns = waveformColumns(pairs, 512, 1024, 0, 2);
    expect(columns[0]).toBeCloseTo(-1, 3); // min of bins 0..1
    expect(columns[1]).toBeCloseTo(200 / 32767, 3);
    expect(columns[2]).toBeCloseTo(-50 / 32767, 3); // min of bins 2..3
    expect(columns[3]).toBeCloseTo(1, 3);
  });

  it('stretches one bin across many columns at deep zoom', () => {
    const pairs = Int16Array.from([-16384, 16384]);
    // fpp 64 = 8 columns per 512-frame bin.
    const columns = waveformColumns(pairs, 512, 64, 0, 8);
    for (let x = 0; x < 8; x += 1) {
      expect(columns[x * 2]).toBeCloseTo(-0.5, 2);
      expect(columns[x * 2 + 1]).toBeCloseTo(0.5, 2);
    }
  });

  it('is flat past the recorded bins and honors the start offset', () => {
    const pairs = Int16Array.from([-100, 100, -200, 200]);
    const columns = waveformColumns(pairs, 512, 512, 512, 3);
    expect(columns[0]).toBeCloseTo(-200 / 32767, 4); // starts at bin 1
    expect(columns[2]).toBe(0); // bin 2 does not exist
    expect(columns[3]).toBe(0);
  });
});
