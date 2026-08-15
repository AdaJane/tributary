/**
 * Grouping and explaining the MIDI half of the patch bay. Pure, for the
 * same reason `output-devices.ts` is: the sentence a user reads about a
 * route that plays nothing is worth testing without a synthesiser plugged
 * in.
 *
 * The MIDI bay is drawn by PORT with routes listed under it, where the
 * audio bay is drawn by CHANNEL with one patch on each. That difference is
 * the merge: an audio output carries one signal because two would have to
 * be summed, and a MIDI port carries any number because events interleave.
 */

import type { components } from '../../api/generated/schema';
import type { InstrumentState } from '../../ws/messages';
import type { SelectOption } from '../../design/select-options';

export type MidiRoute = components['schemas']['MidiRoute'];
export type MidiSource = components['schemas']['MidiSource'];
export type MidiRouteReport = components['schemas']['MidiRouteReport'];
export type MidiRouteStatus = components['schemas']['MidiRouteStatus'];
export type MidiOutPortReport = components['schemas']['MidiOutPortReport'];
export type MidiPortReport = components['schemas']['MidiPortReport'];

/** `None` on the wire: pass the source's own channel through. */
export const PASS_THROUGH = 'pass';

/** One route, joined with its report and its position in the document. */
export interface MidiRouteRow {
  /** Index into the document's `routes`, which is the route's identity
   * for the detail strip — a port alone cannot name one, because a merge
   * puts several on the same port. */
  index: number;
  route: MidiRoute;
  status: MidiRouteStatus;
  reason: string | null;
}

export interface MidiSection {
  port: string;
  title: string;
  /** A route names it but the system does not offer it. */
  absent: boolean;
  sent: number;
  errors: number;
  rows: MidiRouteRow[];
}

/**
 * One section per MIDI output port, in the daemon's order.
 *
 * `routes` and `reports` are joined BY INDEX, which is the contract the
 * daemon states on `MidiDto.reports`. Port is not a key here: a merge puts
 * two routes on one port and they can fail for different reasons — a thru
 * whose keyboard is unplugged beside an instrument echo that is live.
 */
export function groupMidiBay(
  routes: readonly MidiRoute[],
  reports: readonly MidiRouteReport[],
  outputs: readonly MidiOutPortReport[],
): MidiSection[] {
  const rows = routes.map((route, index) => ({
    index,
    route,
    status: reports[index]?.status ?? ('missing' as MidiRouteStatus),
    reason: reports[index]?.reason ?? null,
  }));
  return outputs.map((port) => ({
    port: port.name,
    title: port.name,
    absent: port.absent,
    sent: port.sent,
    errors: port.errors,
    rows: rows.filter((row) => row.route.port === port.name),
  }));
}

/** What a port's counters say, or null when there is nothing to report. */
export function trafficLine(section: MidiSection): string | null {
  if (section.errors > 0) {
    return `${section.errors} write${section.errors === 1 ? '' : 's'} refused`;
  }
  if (section.rows.length === 0) return null;
  // Zero sent is the load-bearing case: a MIDI port has no meter, so
  // without this a dead cable and a quiet keyboard look identical.
  return section.sent === 0 ? 'nothing sent yet' : `${section.sent} sent`;
}

/** The stable select value for one MIDI source. */
export function midiSourceValue(source: MidiSource): string {
  return source.kind === 'instrument'
    ? `instrument:${source.id}`
    : `${source.kind}:${source.name}`;
}

/**
 * A MIDI source's print.
 *
 * The kind is spelt out rather than left to the reader: `nanoKEY2` alone
 * would not say whether it means the keyboard's own notes or a take
 * recorded from it, and those go to the same jack sounding different.
 */
export function midiSourceLabel(source: MidiSource, instruments: readonly InstrumentState[]): string {
  if (source.kind === 'instrument') {
    const name = instruments.find((i) => i.id === source.id)?.name;
    return `${name ?? `Instrument ${source.id + 1}`} (echo)`;
  }
  return `${source.name} (${source.kind === 'port' ? 'thru' : 'take'})`;
}

/**
 * Every source a MIDI output may take — and never a mixer channel.
 *
 * A direct out carries audio and a MIDI port carries events; offering a
 * strip here would be offering a conversion the box cannot do. The list is
 * the three real answers: echo what an instrument is playing, forward a
 * keyboard, or stream a take's sidecar back out.
 */
export function midiSources(
  instruments: readonly InstrumentState[],
  inputs: readonly MidiPortReport[],
  takeTracks: readonly string[],
): MidiSource[] {
  return [
    ...instruments.map((i): MidiSource => ({ kind: 'instrument', id: i.id })),
    ...inputs.map((p): MidiSource => ({ kind: 'port', name: p.name })),
    ...takeTracks.map((name): MidiSource => ({ kind: 'take', name })),
  ];
}

/** The same list as options for a `SelectField`. */
export function midiSourceOptions(
  instruments: readonly InstrumentState[],
  inputs: readonly MidiPortReport[],
  takeTracks: readonly string[],
): SelectOption[] {
  return midiSources(instruments, inputs, takeTracks).map((source) => ({
    value: midiSourceValue(source),
    label: midiSourceLabel(source, instruments),
  }));
}

/** Turn a select value back into the source it names. */
export function midiSourceFromValue(
  value: string,
  instruments: readonly InstrumentState[],
  inputs: readonly MidiPortReport[],
  takeTracks: readonly string[],
): MidiSource | null {
  return (
    midiSources(instruments, inputs, takeTracks).find(
      (source) => midiSourceValue(source) === value,
    ) ?? null
  );
}

/** The forced-channel picker: pass-through, then the sixteen channels. */
export function channelOptions(): SelectOption[] {
  return [
    { value: PASS_THROUGH, label: 'Pass through' },
    ...Array.from({ length: 16 }, (_, i) => ({ value: String(i), label: `Ch ${i + 1}` })),
  ];
}

/** What forcing a channel — or not — will do, said plainly under the picker. */
export function channelConsequence(channel: number | null): string {
  return channel === null
    ? 'every message keeps the channel it arrived on'
    : `every message is forced onto channel ${channel + 1}`;
}

/**
 * Why the tap switch is dead on a MIDI row.
 *
 * Shown disabled with this beside it rather than hidden: a control that
 * vanishes teaches nothing, and "why does the vocal have a pre/post switch
 * and the Juno not" is a fair question with a real answer.
 */
export const MIDI_TAP_REASON =
  'pre/post is a level tap — MIDI carries notes, not a signal to tap';

/** The print on a route row's status lamp. */
export const MIDI_STATUS_LABEL: Record<MidiRouteStatus, string> = {
  live: 'live',
  ready: 'ready',
  missing: 'missing',
};
