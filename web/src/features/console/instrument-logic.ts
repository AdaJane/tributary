/**
 * Instrument presentation logic: what a rack unit is called, why it is
 * silent, and what the patchbay should say about it.
 *
 * Pure, and tested, because these are the sentences that decide whether a
 * quiet channel can be explained. This console has repeatedly lost days to
 * an input that looked healthy and produced nothing; an instrument has
 * four separate ways to be silent, and each one needs its own words.
 */
import type { InstrumentState } from '../../ws/messages';
import type { InputSource } from './input-source';

export interface InstrumentReport {
  id: number;
  name: string;
  status: 'live' | 'ready' | 'missing' | 'failed';
  reason?: string | null;
  presets?: number | null;
  resident_bytes?: number | null;
}

/** Mixer channels an instrument occupies — mirrors the daemon's rule. */
export function instrumentChannels(instrument: InstrumentState): number {
  // `splits` is omitted on the wire when empty — an unsplit instrument is
  // the common case and the manifest stays quiet about it.
  const splits = instrument.splits ?? [];
  return splits.length === 0 ? 2 : splits.length;
}

/** What each of an instrument's channels is called, in channel order. */
export function channelNames(instrument: InstrumentState): string[] {
  const splits = instrument.splits ?? [];
  if (splits.length === 0) {
    return [`${instrument.name} L`, `${instrument.name} R`];
  }
  return splits.map((split) => `${instrument.name} ${split.name}`);
}

/**
 * Why this instrument will not sound, in the daemon's own words where it
 * has them.
 *
 * The daemon's `reason` is preferred over anything derived here: it knows
 * whether a file actually loaded, and a second opinion computed from the
 * document would eventually contradict it.
 */
export function silenceReason(
  instrument: InstrumentState,
  report: InstrumentReport | undefined,
): string | null {
  if (report?.reason) return report.reason;
  if (!instrument.soundfont) return 'no soundfont chosen';
  if (!instrument.port) return 'no MIDI input chosen';
  return null;
}

/** The patchbay says the same thing, and points at where to fix it. */
export function patchbaySilence(
  instrument: InstrumentState,
  report: InstrumentReport | undefined,
): string | null {
  const reason = silenceReason(instrument, report);
  return reason === null ? null : `${reason} — fix it in Instruments`;
}

/** Which strips a given instrument channel feeds, by name. */
export function feedsLabel(
  instrument: InstrumentState,
  strips: readonly { name: string; input?: unknown }[],
  sources: readonly (InputSource | null)[],
): string {
  const fed = strips
    .filter((_, i) => {
      const source = sources[i];
      return source?.kind === 'instrument' && source.instrument === instrument.id;
    })
    .map((s) => s.name);
  if (fed.length === 0) {
    return 'not patched — patch it from a channel’s INPUT button';
  }
  return `feeding ${fed.join(' · ')}`;
}

/**
 * The sentence a screen reader hears for a strip's patch.
 *
 * An instrument patch names the instrument, not its slot number: "INST 2.1"
 * is the right print on a 64px button and the wrong thing to say out loud.
 */
export function patchDescription(
  source: InputSource | null,
  letters: Map<string | null, string>,
  instruments: readonly InstrumentState[],
): string {
  if (source === null) return 'none';
  if (source.kind === 'instrument') {
    const instrument = instruments.find((i) => i.id === source.instrument);
    if (!instrument) return `instrument ${source.instrument + 1} — removed`;
    const names = channelNames(instrument);
    return names[source.channel] ?? `${instrument.name} channel ${source.channel + 1}`;
  }
  const letter = source.device === null ? 'A' : (letters.get(source.device) ?? '?');
  const jack = source.device === null ? `IN ${source.channel + 1}` : `${letter}${source.channel + 1}`;
  return source.device ? `${jack} on ${source.device}` : jack;
}

/** Why "add all channels to mixer" cannot run, or null when it can. */
export function addStripsBlocked(
  instrument: InstrumentState,
  recording: boolean,
  stripCount: number,
  maxStrips: number,
): string | null {
  if (recording) return 'stop recording first';
  const needed = instrumentChannels(instrument);
  if (stripCount + needed > maxStrips) return 'the console is full';
  return null;
}

/** Why an instrument's voice settings cannot be changed right now. */
export function voiceBlocked(recording: boolean): string | null {
  return recording ? 'stop recording first' : null;
}

/** Turn a failed instrument request into a sentence, in the console's
 * dialect — the same table `saveErrorMessage` uses for settings. */
export function instrumentErrorMessage(status: number, detail?: string): string {
  if (status === 409) return detail ?? 'locked while recording';
  if (status === 422) return detail ?? 'the daemon refused that value';
  if (status === 404) return 'that instrument is no longer there';
  return detail ?? 'the daemon could not be reached';
}
