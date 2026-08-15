import { describe, expect, it } from 'vitest';

import type { InstrumentState } from '../../ws/messages';
import type {
  MidiOutPortReport,
  MidiPortReport,
  MidiRoute,
  MidiRouteReport,
  MidiSource,
} from './midi-routes';
import {
  channelConsequence,
  channelOptions,
  groupMidiBay,
  midiSourceFromValue,
  midiSourceLabel,
  midiSourceOptions,
  midiSourceValue,
  trafficLine,
} from './midi-routes';

function instrument(id: number, name: string): InstrumentState {
  return {
    id,
    name,
    bank: 0,
    program: 0,
    polyphony: 32,
    effects: false,
    splits: [],
  } as unknown as InstrumentState;
}

function outPort(name: string, extra: Partial<MidiOutPortReport> = {}): MidiOutPortReport {
  return {
    id: name,
    name,
    connected: true,
    absent: false,
    routes: 0,
    sent: 0,
    errors: 0,
    ...extra,
  };
}

function route(port: string, source: MidiSource, channel: number | null = null): MidiRoute {
  return { port, source, channel };
}

function report(port: string, status: MidiRouteReport['status'], reason?: string): MidiRouteReport {
  return { port, status, reason: reason ?? null };
}

describe('groupMidiBay', () => {
  it('puts two routes on one port under one section — that is the merge', () => {
    const routes = [
      route('Juno', { kind: 'instrument', id: 0 }),
      route('Juno', { kind: 'port', name: 'nanoKEY2' }),
      route('Blofeld', { kind: 'take', name: 'Kit Kick' }),
    ];
    const sections = groupMidiBay(
      routes,
      [report('Juno', 'live'), report('Juno', 'ready'), report('Blofeld', 'live')],
      [outPort('Juno'), outPort('Blofeld')],
    );
    expect(sections.map((s) => s.rows.length)).toEqual([2, 1]);
  });

  it('joins reports by index, not by port, so a merge can fail one route at a time', () => {
    // Port alone is not a key here. Joining on it would give the live echo
    // the unplugged keyboard's excuse, and vice versa.
    const routes = [
      route('Juno', { kind: 'instrument', id: 0 }),
      route('Juno', { kind: 'port', name: 'nanoKEY2' }),
    ];
    const [juno] = groupMidiBay(
      routes,
      [report('Juno', 'live'), report('Juno', 'ready', 'its input “nanoKEY2” is not connected')],
      [outPort('Juno')],
    );
    expect(juno.rows[0].status).toBe('live');
    expect(juno.rows[0].reason).toBeNull();
    expect(juno.rows[1].status).toBe('ready');
    expect(juno.rows[1].reason).toContain('nanoKEY2');
  });

  it('keeps a route pointing at a port that is gone, so its silence is explainable', () => {
    const routes = [route('Juno', { kind: 'instrument', id: 0 })];
    const sections = groupMidiBay(
      routes,
      [report('Juno', 'missing', '“Juno” is not connected')],
      [outPort('Juno', { absent: true, connected: false, routes: 1 })],
    );
    expect(sections[0].absent).toBe(true);
    expect(sections[0].rows).toHaveLength(1);
  });

  it('carries a row for a port with no routes, so there is somewhere to patch', () => {
    expect(groupMidiBay([], [], [outPort('Juno')])).toEqual([
      { port: 'Juno', title: 'Juno', absent: false, sent: 0, errors: 0, rows: [] },
    ]);
  });
});

describe('trafficLine', () => {
  const section = (extra: Partial<MidiOutPortReport>) =>
    groupMidiBay(
      [route('Juno', { kind: 'instrument', id: 0 })],
      [report('Juno', 'live')],
      [outPort('Juno', extra)],
    )[0];

  it('says nothing has been sent, because a MIDI port has no meter', () => {
    // A dead cable and a quiet keyboard look identical without this.
    expect(trafficLine(section({ sent: 0 }))).toBe('nothing sent yet');
  });

  it('counts what went out once something has', () => {
    expect(trafficLine(section({ sent: 412 }))).toBe('412 sent');
  });

  it('reports refused writes ahead of the count', () => {
    expect(trafficLine(section({ sent: 412, errors: 3 }))).toBe('3 writes refused');
  });

  it('says nothing at all about a port nobody has routed', () => {
    expect(trafficLine(groupMidiBay([], [], [outPort('Juno')])[0])).toBeNull();
  });
});

describe('midi sources', () => {
  const rack = [instrument(0, 'Rhodes'), instrument(1, 'Strings')];
  const inputs: MidiPortReport[] = [
    { id: 'nanoKEY2', name: 'nanoKEY2', connected: true, absent: false },
  ];

  it('offers instruments, inputs and take tracks — and never a mixer channel', () => {
    // A direct out carries audio; a MIDI port carries events. Offering a
    // strip here would be offering a conversion the box cannot do.
    const options = midiSourceOptions(rack, inputs, ['Kit Kick']);
    expect(options.map((o) => o.label)).toEqual([
      'Rhodes (echo)',
      'Strings (echo)',
      'nanoKEY2 (thru)',
      'Kit Kick (take)',
    ]);
  });

  it('spells the kind out, because the same name means different things', () => {
    // "nanoKEY2" alone would not say whether it is the keyboard playing now
    // or a take recorded from it.
    expect(midiSourceLabel({ kind: 'port', name: 'nanoKEY2' }, rack)).toBe('nanoKEY2 (thru)');
    expect(midiSourceLabel({ kind: 'take', name: 'nanoKEY2' }, rack)).toBe('nanoKEY2 (take)');
  });

  it('names an instrument the rack no longer holds rather than printing nothing', () => {
    expect(midiSourceLabel({ kind: 'instrument', id: 6 }, rack)).toBe('Instrument 7 (echo)');
  });

  it('round-trips every source through its select value', () => {
    for (const option of midiSourceOptions(rack, inputs, ['Kit Kick'])) {
      const source = midiSourceFromValue(option.value, rack, inputs, ['Kit Kick']);
      expect(source).not.toBeNull();
      expect(midiSourceValue(source!)).toBe(option.value);
    }
  });

  it('refuses a value naming something that is gone', () => {
    expect(midiSourceFromValue('take:Kit Snare', rack, inputs, ['Kit Kick'])).toBeNull();
  });
});

describe('channel forcing', () => {
  it('prints channels one-based, as every synth does', () => {
    const options = channelOptions();
    expect(options[0]).toEqual({ value: 'pass', label: 'Pass through' });
    expect(options[1]).toEqual({ value: '0', label: 'Ch 1' });
    expect(options.at(-1)).toEqual({ value: '15', label: 'Ch 16' });
  });

  it('says what forcing does in the same one-based terms', () => {
    expect(channelConsequence(9)).toContain('channel 10');
    expect(channelConsequence(null)).toContain('keeps the channel it arrived on');
  });
});
