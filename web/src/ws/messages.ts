/**
 * The WS protocol, typed from the daemon's OpenAPI components. The
 * exhaustive handler map makes an unhandled server message a COMPILE error
 * rather than a silently dropped frame.
 */
import type { components } from '../api/generated/schema';

export type ServerMessage = components['schemas']['ServerMessage'];
export type ClientMessage = components['schemas']['ClientMessage'];
export type Channel = components['schemas']['Channel'];
export type MixCommand = components['schemas']['MixCommand'];
export type StateDelta = components['schemas']['StateDelta'];
export type MixerState = components['schemas']['MixerState'];
export type StripState = components['schemas']['StripState'];
export type MeterDto = components['schemas']['MeterDto'];
export type MeterKey = components['schemas']['MeterKey'];
export type WsAck = components['schemas']['WsAck'];
export type TransportDto = components['schemas']['TransportDto'];

/** Compile-time drift trap: adding a ServerMessage variant on the daemon
 * without deciding its client handling breaks the build here. */
export const SERVER_MESSAGE_TYPES: Record<ServerMessage['type'], true> = {
  mixer_snapshot: true,
  state_changed: true,
  meters: true,
  transport: true,
  playback_position: true,
  waveform_bins: true,
  subscribed: true,
  unsubscribed: true,
  error: true,
  pong: true,
};

/** Stable identity for a channel (subscription refcounts, routing). */
export function channelKey(channel: Channel): string {
  return channel.kind;
}

/** Stable identity for a metered point. */
export function meterKeyString(key: MeterKey): string {
  switch (key.kind) {
    case 'strip':
      return `strip:${key.id}`;
    case 'bus':
      return `bus:${key.id}`;
    case 'master':
      return 'master';
  }
}

export function parseServerMessage(raw: string): ServerMessage | null {
  try {
    const parsed = JSON.parse(raw) as ServerMessage;
    return parsed.type in SERVER_MESSAGE_TYPES ? parsed : null;
  } catch {
    return null;
  }
}
