/**
 * What a waveform lane is called. Extracted from `Timeline` so it can be
 * tested — it was the only untested logic left in a Tracks component.
 */

export interface LaneMeta {
  file: string;
  channels: number;
  stripId: number | null;
}

export interface NamedStrip {
  id: number;
  name: string;
}

/**
 * Resolve a lane's name, best effort.
 *
 * `strip_id` is the real association and outlives filename conventions.
 * It resolves against the LIVE console, so renaming a strip retroactively
 * relabels an old take's lane — a known wart, not fixed here.
 *
 * Takes cut before `strip_id` existed fall back to the `chNN-slug`
 * filename. Note both container extensions are stripped: stripping only
 * `.wav` made a legacy FLAC take render as "kick.flac".
 */
export function laneName(meta: LaneMeta | undefined, strips: readonly NamedStrip[]): string {
  if (!meta) return '—';
  if (meta.channels === 2) return 'Master';
  if (meta.stripId !== null) {
    const strip = strips.find((s) => s.id === meta.stripId);
    if (strip) return strip.name;
  }
  const stem = meta.file.replace(/\.(wav|flac)$/i, '');
  return stem.replace(/^ch\d+-/, '') || stem;
}
