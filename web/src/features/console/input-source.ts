/**
 * What a strip is patched to, as one thing the rest of the console can ask
 * questions of.
 *
 * The daemon's `InputAssign` is a union — a hardware jack or an instrument
 * channel — and the two shapes have different field names on the wire. Every
 * place that used to read `.device` now goes through here instead, so the
 * union is narrowed once, in one tested module, rather than at a dozen call
 * sites that would each have to remember the other case exists.
 */
import type { InputAssign } from '../../ws/messages';

/** A hardware patch: a device by OS name (`null` = the system default). */
export interface DeviceSource {
  kind: 'device';
  device: string | null;
  channel: number;
}

/** An instrument patch: a rack unit and a channel within it. */
export interface InstrumentSource {
  kind: 'instrument';
  instrument: number;
  channel: number;
}

export type InputSource = DeviceSource | InstrumentSource;

/** Narrow a wire patch. `null`/absent = unpatched. */
export function inputSource(assign: InputAssign | null | undefined): InputSource | null {
  if (!assign) return null;
  if ('instrument' in assign) {
    return { kind: 'instrument', instrument: assign.instrument, channel: assign.channel };
  }
  return {
    kind: 'device',
    device: assign.device ?? null,
    channel: assign.device_channel,
  };
}

/**
 * The REST body shape.
 *
 * `PUT /strips/{id}` takes a flattened object with all three fields
 * optional — it refuses naming both a device and an instrument server-side
 * — whereas the document's `InputAssign` is a proper union. One converter
 * rather than two shapes leaking through the console.
 */
export interface InputAssignBody {
  device?: string | null;
  instrument?: number | null;
  channel: number;
}

export function toBody(source: InputSource | null): InputAssignBody | null {
  if (source === null) return null;
  return source.kind === 'device'
    ? { device: source.device, channel: source.channel }
    : { instrument: source.instrument, channel: source.channel };
}

/** The wire shape for a device patch. */
export function deviceAssign(device: string | null, channel: number): InputAssign {
  return device === null
    ? { device_channel: channel }
    : { device, device_channel: channel };
}

/** The wire shape for an instrument patch. */
export function instrumentAssign(instrument: number, channel: number): InputAssign {
  return { instrument, channel };
}

/**
 * Stable string identity for whatever feeds a strip.
 *
 * Two devices' channel 0 are different jacks, and so are a device's and an
 * instrument's — hence the prefix. This is what keys the link tape's
 * colour, so it has to separate anything a person would call a different
 * source.
 */
export function sourceKey(source: InputSource): string {
  return source.kind === 'device'
    ? `dev:${source.device ?? ''}#${source.channel}`
    : `inst:${source.instrument}#${source.channel}`;
}

export function sameSource(a: InputSource | null, b: InputSource | null): boolean {
  if (a === null || b === null) return a === b;
  return sourceKey(a) === sourceKey(b);
}
