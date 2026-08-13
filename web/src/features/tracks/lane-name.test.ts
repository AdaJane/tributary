import { describe, expect, it } from 'vitest';

import { laneName } from './lane-name';
import type { LaneMeta } from './lane-name';

const STRIPS = [
  { id: 0, name: 'Kick' },
  { id: 4, name: 'Vox' },
];

function meta(over: Partial<LaneMeta> = {}): LaneMeta {
  return { file: 'ch01-kick.wav', channels: 1, stripId: 0, ...over };
}

describe('laneName', () => {
  it('prefers the strip a track tapped', () => {
    expect(laneName(meta({ stripId: 4 }), STRIPS)).toBe('Vox');
  });

  it('calls the stereo track Master', () => {
    expect(laneName(meta({ channels: 2, stripId: null }), STRIPS)).toBe('Master');
  });

  it('recovers a name from the filename when the take predates strip_id', () => {
    expect(laneName(meta({ stripId: null, file: 'ch03-snare.wav' }), STRIPS)).toBe('snare');
  });

  /** Stripping only `.wav` made a legacy FLAC take render as "kick.flac". */
  it('strips both container extensions, not just wav', () => {
    expect(laneName(meta({ stripId: null, file: 'ch02-hat.flac' }), STRIPS)).toBe('hat');
    expect(laneName(meta({ stripId: null, file: 'ch02-hat.FLAC' }), STRIPS)).toBe('hat');
  });

  it('falls back to the stem when a strip_id names nothing live', () => {
    expect(laneName(meta({ stripId: 99, file: 'ch09-gone.wav' }), STRIPS)).toBe('gone');
  });

  it('renders an em dash for a lane with no metadata yet', () => {
    expect(laneName(undefined, STRIPS)).toBe('—');
  });
});
