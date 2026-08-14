/**
 * Output patch identity and shape. Pure — the store maps wire shapes, this
 * decides what a patch IS.
 *
 * The cardinality is inverted from the input side and that inversion runs
 * through the whole room: on the way in a STRIP is the "one" and jacks are
 * the "many"; on the way out an OUTPUT CHANNEL is the "one" and sources are
 * the many. A jack holds at most one feed.
 */

import type { components } from '../../api/generated/schema';

export type OutputPatch = components['schemas']['OutputPatch'];
export type OutputSource = components['schemas']['OutputSource'];
export type OutputDeviceReport = components['schemas']['OutputDeviceReport'];
export type OutputPatchReport = components['schemas']['OutputPatchReport'];
/** The daemon's own words: `pre_fader` | `post_fader`. */
export type PatchTap = OutputPatch['tap'];

/**
 * A jack's identity: `(device, channel)`.
 *
 * Prefixed `out:` so it can never collide with an input jack key — both are
 * `(device, channel)` pairs, and a shared prefix would let a future map
 * confuse a patch with a jack and hand them the same tape colour.
 */
export function outputKey(device: string | null | undefined, channel: number): string {
  return `out:${device ?? ''}#${channel}`;
}

export function sourceKey(source: OutputSource, channel: number): string {
  return source.kind === 'master'
    ? `master#${channel}`
    : `${source.kind}:${source.id}#${channel}`;
}

export function sameSource(a: OutputSource, b: OutputSource): boolean {
  if (a.kind === 'master' || b.kind === 'master') return a.kind === b.kind;
  return a.kind === b.kind && a.id === b.id;
}

/** The patch on a jack, if any. */
export function patchAt(
  patches: readonly OutputPatch[],
  device: string | null,
  channel: number,
): OutputPatch | null {
  const key = outputKey(device, channel);
  return patches.find((p) => outputKey(p.device ?? null, p.channel) === key) ?? null;
}

/** Every jack a source feeds, in device then channel order. */
export function jacksFedBy(
  patches: readonly OutputPatch[],
  source: OutputSource,
): OutputPatch[] {
  return patches.filter((p) => sameSource(p.source, source));
}
