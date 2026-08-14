/**
 * Instruments as patchbay sections.
 *
 * They are drawn beside the lettered stage boxes but never lettered
 * themselves: a box `I` and a rack slot `I1` would collide in print, and —
 * worse — lettering instruments would mean adding one re-letters the stage
 * boxes, changing the colour and label of the tape on two strips under the
 * user's hands. Hardware keeps A, B, C…; racks print `INST 3`.
 */
import type { InstrumentState, StripState } from '../../ws/messages';
import type { InstrumentReport } from './instrument-logic';
import { channelNames, instrumentChannels, patchbaySilence } from './instrument-logic';
import { inputSource } from './input-source';

export interface InstrumentSection {
  id: number;
  /** `INST 3` — the slot chip that stands where a letter would. */
  slot: string;
  title: string;
  /** One tile per mixer channel, named as the strip would be. */
  channels: string[];
  status: 'live' | 'ready' | 'missing' | 'failed';
  /** Why it will not sound, already suffixed with where to fix it. */
  silence: string | null;
  /** This instrument is referenced by a strip but no longer in the rack. */
  ghost: boolean;
}

const STATUS_FOR_GHOST = 'missing' as const;

/**
 * Sections for every instrument in the rack, plus a ghost for any a strip
 * still points at.
 *
 * The ghost is the important half. Deleting an instrument that fed a strip
 * would otherwise leave the strip's INPUT button printing `INST 3.1` with
 * nothing in the patchbay to explain it — a silent channel with no story,
 * which is precisely what this modal exists to prevent.
 */
export function instrumentSections(
  instruments: readonly InstrumentState[],
  reports: readonly InstrumentReport[],
  strips: readonly StripState[],
): InstrumentSection[] {
  const sections: InstrumentSection[] = instruments.map((instrument) => {
    const report = reports.find((r) => r.id === instrument.id);
    return {
      id: instrument.id,
      slot: `INST ${instrument.id + 1}`,
      title: instrument.name,
      channels: channelNames(instrument),
      status: report?.status ?? 'ready',
      silence: patchbaySilence(instrument, report),
      ghost: false,
    };
  });

  const known = new Set(instruments.map((i) => i.id));
  const orphans = new Set<number>();
  for (const strip of strips) {
    const source = inputSource(strip.input);
    if (source?.kind === 'instrument' && !known.has(source.instrument)) {
      orphans.add(source.instrument);
    }
  }
  for (const id of [...orphans].sort((a, b) => a - b)) {
    sections.push({
      id,
      slot: `INST ${id + 1}`,
      title: `Instrument ${id + 1}`,
      channels: [],
      status: STATUS_FOR_GHOST,
      silence: 'this instrument was removed — patch this channel somewhere else',
      ghost: true,
    });
  }
  return sections;
}

/** Strips already fed by this instrument channel, by name. */
export function holdersOf(
  strips: readonly StripState[],
  instrument: number,
  channel: number,
): string[] {
  return strips
    .filter((strip) => {
      const source = inputSource(strip.input);
      return (
        source?.kind === 'instrument' &&
        source.instrument === instrument &&
        source.channel === channel
      );
    })
    .map((strip) => strip.name);
}

/** How many channels the rack currently occupies — for the budget line. */
export function rackChannels(instruments: readonly InstrumentState[]): number {
  return instruments.reduce((total, i) => total + instrumentChannels(i), 0);
}
