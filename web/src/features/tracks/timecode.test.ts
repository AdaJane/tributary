import { describe, expect, it } from 'vitest';

import { timecode } from './timecode';

describe('timecode', () => {
  it('renders zero as 0:00.0', () => {
    expect(timecode(0, 48_000)).toBe('0:00.0');
  });

  it('renders whole seconds and tenths from frames', () => {
    expect(timecode(48_000, 48_000)).toBe('0:01.0');
    expect(timecode(48_000 * 4.25, 48_000)).toBe('0:04.2');
  });

  it('rolls seconds into minutes past sixty', () => {
    expect(timecode(48_000 * 93, 48_000)).toBe('1:33.0');
    expect(timecode(48_000 * 600, 48_000)).toBe('10:00.0');
  });

  it('is defensive about garbage inputs', () => {
    expect(timecode(-5, 48_000)).toBe('0:00.0');
    expect(timecode(100, 0)).toBe('0:00.0');
  });
});
