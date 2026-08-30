import { describe, expect, it } from 'vitest';

import {
  parsePresetValue,
  presetFallback,
  presetOptions,
  presetValue,
} from './preset-options';

/** Real names and numbers from GeneralUser GS, read off the daemon. */
const GM = [
  { bank: 0, program: 40, name: 'Violin' },
  { bank: 0, program: 0, name: 'Grand Piano' },
  { bank: 8, program: 0, name: 'Piano (wide)' },
  { bank: 0, program: 73, name: 'Flute' },
];

describe('preset addressing', () => {
  /** A preset is a (bank, program) pair and a select value is one string.
   *  Getting this round trip wrong loads the wrong sound silently, which
   *  is why it is a pure function with a test rather than inline JSX. */
  it('round-trips through the select value', () => {
    for (const p of GM) {
      expect(parsePresetValue(presetValue(p.bank, p.program))).toEqual({
        bank: p.bank,
        program: p.program,
      });
    }
  });

  /** Nothing, rather than a plausible-looking bank 0 program 0. */
  it('refuses anything it did not produce', () => {
    for (const bad of ['', '40', 'a:b', '0:', ':0', '0:128', '16384:0', '0:1:2']) {
      expect(parsePresetValue(bad)).toBeNull();
    }
  });
});

describe('presetOptions', () => {
  /** General MIDI order — the order a player scrolls, not the order the
   *  file happens to store them in. */
  it('sorts by bank then program', () => {
    expect(presetOptions(GM).map((o) => o.value)).toEqual(['0:0', '0:40', '0:73', '8:0']);
  });

  /** The number stays visible: it is what a MIDI file and a hardware
   *  controller both speak, so a name alone leaves "program 40"
   *  unanswerable. */
  it('shows the program number beside the name', () => {
    const violin = presetOptions(GM).find((o) => o.value === '0:40');
    expect(violin?.label).toBe('040  Violin');
  });

  /** Bank only when it is not the General MIDI one, so the common case
   *  does not read as a coordinate pair. */
  it('names the bank only when it is not bank 0', () => {
    const wide = presetOptions(GM).find((o) => o.value === '8:0');
    expect(wide?.label).toBe('8:000  Piano (wide)');
  });
});

describe('presetFallback', () => {
  /** While the list loads — or if the file cannot be read — the picker
   *  still shows what the instrument actually holds. An empty box would
   *  suggest the instrument has no preset, which is never true. */
  it('shows the numbers the instrument holds', () => {
    expect(presetFallback(0, 40)).toEqual([{ value: '0:40', label: '0:040' }]);
  });
});
