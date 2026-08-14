import { describe, expect, it } from 'vitest';

import type { InstrumentState, StripState } from '../../ws/messages';
import { holdersOf, instrumentSections, rackChannels } from './instrument-sections';

function instrument(id: number, name: string, splits?: InstrumentState['splits']) {
  return {
    id,
    name,
    bank: 0,
    program: 0,
    polyphony: 32,
    effects: false,
    soundfont: 'piano.sf2',
    port: 'nanoKEY2 MIDI 1',
    ...(splits ? { splits } : {}),
  } as InstrumentState;
}

function strip(id: number, name: string, input: StripState['input']): StripState {
  return { id, name, input } as StripState;
}

describe('instrument sections', () => {
  it('prints a slot chip instead of a stage-box letter', () => {
    // Lettering instruments would collide with the stage boxes AND make
    // adding one re-letter them, changing the colour and print of the tape
    // on two strips under the user's hands.
    const [section] = instrumentSections([instrument(2, 'Rhodes')], [], []);
    expect(section.slot).toBe('INST 3');
    expect(section.channels).toEqual(['Rhodes L', 'Rhodes R']);
  });

  it('a split kit offers one tile per drum', () => {
    const kit = instrument(0, 'Kit', [
      { name: 'Kick', ranges: [[35, 36]] },
      { name: 'Snare', ranges: [[37, 40]] },
      { name: 'HiHat', ranges: [[42, 42]] },
    ]);
    const [section] = instrumentSections([kit], [], []);
    expect(section.channels).toEqual(['Kit Kick', 'Kit Snare', 'Kit HiHat']);
  });

  it('a removed instrument a strip still points at gets a ghost section', () => {
    // Otherwise the strip's INPUT button prints INST 4.1 with nothing in
    // the patchbay to explain it — a silent channel with no story, which
    // is precisely what this modal exists to prevent.
    const strips = [strip(0, 'Keys', { instrument: 3, channel: 0 })];
    const sections = instrumentSections([], [], strips);
    expect(sections).toHaveLength(1);
    expect(sections[0].ghost).toBe(true);
    expect(sections[0].status).toBe('missing');
    expect(sections[0].silence).toContain('was removed');
  });

  it('an instrument that is present gets no ghost', () => {
    const strips = [strip(0, 'Keys', { instrument: 0, channel: 0 })];
    const sections = instrumentSections([instrument(0, 'Rhodes')], [], strips);
    expect(sections).toHaveLength(1);
    expect(sections[0].ghost).toBe(false);
  });

  it('a device patch never produces a ghost', () => {
    const strips = [strip(0, 'Mic', { device_channel: 0 })];
    expect(instrumentSections([], [], strips)).toHaveLength(0);
  });

  it('carries the daemon’s status and reason onto the section', () => {
    const sections = instrumentSections(
      [instrument(0, 'Rhodes')],
      [{ id: 0, name: 'Rhodes', status: 'failed', reason: 'piano.sf2 could not be read' }],
      [],
    );
    expect(sections[0].status).toBe('failed');
    expect(sections[0].silence).toBe(
      'piano.sf2 could not be read — fix it in Instruments',
    );
  });
});

describe('holders', () => {
  it('names every strip on one instrument channel', () => {
    const strips = [
      strip(0, 'Kick', { instrument: 0, channel: 0 }),
      strip(1, 'Kick dup', { instrument: 0, channel: 0 }),
      strip(2, 'Snare', { instrument: 0, channel: 1 }),
    ];
    expect(holdersOf(strips, 0, 0)).toEqual(['Kick', 'Kick dup']);
    expect(holdersOf(strips, 0, 1)).toEqual(['Snare']);
    expect(holdersOf(strips, 1, 0)).toEqual([]);
  });
});

describe('rack budget', () => {
  it('sums stereo pairs and mono splits', () => {
    const kit = instrument(1, 'Kit', [
      { name: 'Kick', ranges: [[35, 36]] },
      { name: 'Snare', ranges: [[37, 40]] },
    ]);
    expect(rackChannels([instrument(0, 'Rhodes'), kit])).toBe(4);
  });
});
