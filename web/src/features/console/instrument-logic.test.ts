import { describe, expect, it } from 'vitest';

import type { InstrumentState } from '../../ws/messages';
import {
  addStripsBlocked,
  channelNames,
  feedsLabel,
  instrumentChannels,
  instrumentErrorMessage,
  patchDescription,
  patchbaySilence,
  silenceReason,
} from './instrument-logic';
import type { InputSource } from './input-source';

function instrument(over: Partial<InstrumentState> = {}): InstrumentState {
  return {
    id: 0,
    name: 'Rhodes',
    bank: 0,
    program: 0,
    polyphony: 32,
    effects: false,
    soundfont: 'piano.sf2',
    port: 'nanoKEY2 MIDI 1',
    ...over,
  } as InstrumentState;
}

const KIT = instrument({
  id: 1,
  name: 'Kit',
  splits: [
    { name: 'Kick', ranges: [[35, 36]] },
    { name: 'Snare', ranges: [[37, 40]] },
  ],
});

describe('instrument channels', () => {
  it('an unsplit instrument occupies a stereo pair', () => {
    expect(instrumentChannels(instrument())).toBe(2);
    expect(channelNames(instrument())).toEqual(['Rhodes L', 'Rhodes R']);
  });

  it('a split kit occupies one mono channel per piece', () => {
    expect(instrumentChannels(KIT)).toBe(2);
    expect(channelNames(KIT)).toEqual(['Kit Kick', 'Kit Snare']);
  });

  it('an absent splits field is the unsplit case, not a crash', () => {
    // The daemon omits `splits` when empty, which is the common case.
    const bare = { ...instrument() } as InstrumentState;
    delete (bare as { splits?: unknown }).splits;
    expect(instrumentChannels(bare)).toBe(2);
  });
});

describe('why an instrument is silent', () => {
  it('prefers the daemon’s own sentence over anything derived here', () => {
    // The daemon knows whether the file actually loaded; a second opinion
    // computed from the document would eventually contradict it.
    const reason = silenceReason(instrument(), {
      id: 0,
      name: 'Rhodes',
      status: 'failed',
      reason: 'piano.sf2 could not be read',
    });
    expect(reason).toBe('piano.sf2 could not be read');
  });

  it('names the missing soundfont before the missing port', () => {
    expect(silenceReason(instrument({ soundfont: null, port: null }), undefined)).toBe(
      'no soundfont chosen',
    );
    expect(silenceReason(instrument({ port: null }), undefined)).toBe(
      'no MIDI input chosen',
    );
  });

  it('a healthy instrument has nothing to explain', () => {
    expect(silenceReason(instrument(), undefined)).toBeNull();
    expect(patchbaySilence(instrument(), undefined)).toBeNull();
  });

  it('the patchbay says the same thing and points at the fix', () => {
    expect(patchbaySilence(instrument({ soundfont: null }), undefined)).toBe(
      'no soundfont chosen — fix it in Instruments',
    );
  });
});

describe('what a patch is called', () => {
  const letters = new Map<string | null, string>([
    [null, 'A'],
    ['dock', 'B'],
  ]);

  it('an instrument patch is spoken as the channel’s name, not its slot', () => {
    // "INST 2.1" is the right print on a 64px button and the wrong thing
    // to say out loud.
    const source: InputSource = { kind: 'instrument', instrument: 1, channel: 1 };
    expect(patchDescription(source, letters, [KIT])).toBe('Kit Snare');
  });

  it('a patch at a removed instrument says so rather than going blank', () => {
    const source: InputSource = { kind: 'instrument', instrument: 7, channel: 0 };
    expect(patchDescription(source, letters, [KIT])).toContain('removed');
  });

  it('a device patch keeps its stage-box print', () => {
    expect(
      patchDescription({ kind: 'device', device: 'dock', channel: 0 }, letters, []),
    ).toBe('B1 on dock');
    expect(
      patchDescription({ kind: 'device', device: null, channel: 2 }, letters, []),
    ).toBe('IN 3');
  });

  it('an unpatched strip says none', () => {
    expect(patchDescription(null, letters, [])).toBe('none');
  });
});

describe('feeds line', () => {
  const strips = [
    { name: 'Kick' },
    { name: 'Snare' },
    { name: 'Mic' },
  ];
  const sources: (InputSource | null)[] = [
    { kind: 'instrument', instrument: 1, channel: 0 },
    { kind: 'instrument', instrument: 1, channel: 1 },
    { kind: 'device', device: null, channel: 0 },
  ];

  it('names every strip the instrument feeds', () => {
    expect(feedsLabel(KIT, strips, sources)).toBe('feeding Kick · Snare');
  });

  it('an unpatched instrument names the door to fix it', () => {
    // Assignment lives in the patchbay by design, so the rack has to say
    // where to go.
    expect(feedsLabel(instrument(), strips, sources)).toContain('INPUT button');
  });
});

describe('blocked reasons', () => {
  it('recording blocks adding channels', () => {
    expect(addStripsBlocked(KIT, true, 0, 32)).toBe('stop recording first');
  });

  it('a full console blocks adding channels, counting what it needs', () => {
    // Two channels needed, one slot left.
    expect(addStripsBlocked(KIT, false, 31, 32)).toBe('the console is full');
    expect(addStripsBlocked(KIT, false, 30, 32)).toBeNull();
  });
});

describe('error dialect', () => {
  it('speaks the same sentences the settings panel does', () => {
    expect(instrumentErrorMessage(409)).toBe('locked while recording');
    expect(instrumentErrorMessage(409, 'stop recording first')).toBe(
      'stop recording first',
    );
    expect(instrumentErrorMessage(404)).toContain('no longer there');
  });
});
