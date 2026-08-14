/**
 * What a source is called, and which sources an output may take. Pure —
 * the wording a user reads is worth testing without a browser attached.
 */

import type { MixerState } from '../../ws/messages';
import type { OutputPatch, OutputSource, PatchTap } from './output-patch';
import { jacksFedBy, sameSource } from './output-patch';

export interface SourceOption {
  /** Stable select value: `master#0`, `strip:3#0`, `bus:1#1`. */
  value: string;
  label: string;
  source: OutputSource;
  channel: number;
}

/** How wide a source is. The daemon owns this; we mirror it to build the list. */
export function sourceWidth(source: OutputSource, mixer: MixerState): number {
  if (source.kind === 'master') return 2;
  if (source.kind === 'strip') return 1;
  const bus = mixer.buses.find((b) => b.id === source.id);
  // A group is a panned stereo pair; an aux collects mono sends.
  return bus?.kind === 'group' ? 2 : 1;
}

/**
 * A source channel's name, in the console's voice.
 *
 * The master's sides are `Master L` / `Master R`, not `Master 1` — the
 * same choice `channelNames` makes for a stereo instrument, and for the
 * same reason: nobody calls the left side of a mix "channel one".
 */
export function sourceChannelName(
  source: OutputSource,
  channel: number,
  mixer: MixerState,
): string {
  const side = channel === 0 ? 'L' : 'R';
  if (source.kind === 'master') return `Master ${side}`;
  if (source.kind === 'strip') {
    return mixer.strips.find((s) => s.id === source.id)?.name ?? `Strip ${source.id + 1}`;
  }
  const bus = mixer.buses.find((b) => b.id === source.id);
  const name = bus?.name ?? `Bus ${source.id + 1}`;
  return bus?.kind === 'group' ? `${name} ${side}` : name;
}

/**
 * Every source an audio output may take, in desk order: strips, then
 * buses, then the master.
 *
 * The master and the buses appear HERE rather than as objects needing a
 * channel strip of their own — which is what makes a room organised by
 * output able to reach them at all. There are no bus strips in this
 * console.
 */
export function sourceOptions(mixer: MixerState): SourceOption[] {
  const options: SourceOption[] = [];
  const push = (source: OutputSource) => {
    for (let channel = 0; channel < sourceWidth(source, mixer); channel += 1) {
      options.push({
        value:
          source.kind === 'master'
            ? `master#${channel}`
            : `${source.kind}:${source.id}#${channel}`,
        label: sourceChannelName(source, channel, mixer),
        source,
        channel,
      });
    }
  };
  for (const strip of mixer.strips) push({ kind: 'strip', id: strip.id });
  for (const bus of mixer.buses) push({ kind: 'bus', id: bus.id });
  push({ kind: 'master' });
  return options;
}

/** What flipping the tap will do, said plainly under the switch. */
export function tapConsequence(tap: PatchTap): string {
  return tap === 'pre_fader'
    ? 'the fader and mute do not affect this output'
    : 'the fader and mute follow the mix';
}

/**
 * The print on a strip's OUT button: `—`, `OUT 3`, or `2 OUTS`.
 *
 * A count rather than a list past one, because 88px of strip does not hold
 * two labels and a truncated one teaches nothing.
 */
export function outputButtonLabel(
  patches: readonly OutputPatch[],
  source: OutputSource,
): string {
  const fed = jacksFedBy(patches, source);
  if (fed.length === 0) return '—';
  if (fed.length === 1) return `OUT ${fed[0].channel + 1}`;
  return `${fed.length} OUTS`;
}

/** The sentence a screen reader gets instead of the print. */
export function outputDescription(
  patches: readonly OutputPatch[],
  source: OutputSource,
  mixer: MixerState,
): string {
  const fed = jacksFedBy(patches, source);
  if (fed.length === 0) return 'not patched to any output';
  return fed
    .map((patch) => {
      const where = patch.device ? `${patch.device} output ${patch.channel + 1}` : `output ${patch.channel + 1}`;
      const tap = patch.tap === 'pre_fader' ? 'pre-fader' : 'post-fader';
      return `${sourceChannelName(patch.source, patch.source_channel, mixer)} to ${where}, ${tap}`;
    })
    .join('; ');
}

/** Whether `source` already feeds something, for the strip's lamp. */
export function feedsAnything(
  patches: readonly OutputPatch[],
  source: OutputSource,
): boolean {
  return patches.some((p) => sameSource(p.source, source));
}
