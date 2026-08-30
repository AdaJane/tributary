/**
 * Soundfont library presentation: how the list is grouped, what a file's
 * detail line says, and whether an upload can even be attempted.
 *
 * Pure so the copy is testable — and because the upload gate is worth
 * getting right: refusing a 300 MB file after twenty minutes of Wi-Fi is a
 * far worse experience than refusing it before it starts.
 */
import type { SoundfontInfo } from '../../state/instruments';

export const SOUNDFONT_EXT = '.sf2';

export interface SoundfontGroup {
  /** `Built in`, `Internal`, or the volume label of a mounted drive. */
  title: string;
  removable: boolean;
  /** Shipped with the installation: playable, never deletable. */
  builtIn: boolean;
  files: SoundfontInfo[];
}

/**
 * Above this, a font's size is worth saying out loud.
 *
 * rustysynth holds all of a font's sample data resident for as long as it
 * is loaded, so the file size is very nearly the memory cost — and the
 * bundled General MIDI banks run to 142 MB and 206 MB. On a 2 GB Pi 4 that
 * is the difference between a preset change and an OOM kill, which is not
 * something to discover during a take.
 */
export const RAM_NOTICE_BYTES = 64 * 1024 * 1024;

/** A plain-language memory warning for a large font, or null. */
export function ramNotice(bytes: number): string | null {
  if (bytes < RAM_NOTICE_BYTES) return null;
  return `${formatBytes(bytes)} — needs about the same again in memory while loaded`;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/**
 * Built-ins first, then Internal, then one group per drive.
 *
 * Grouping is not disclosure: every file is always rendered. It exists so a
 * font on somebody's stick reads as theirs — which is also why only the
 * internal ones can be deleted from here. Built-ins lead because on a
 * fresh appliance they are the only sounds there are, and an empty
 * "Internal" heading above them would read as a box with nothing in it.
 */
export function groupSoundfonts(files: readonly SoundfontInfo[]): SoundfontGroup[] {
  const builtIn = files.filter((f) => f.origin === 'built_in');
  const internal = files.filter((f) => f.origin === 'internal');
  const groups: SoundfontGroup[] = [];
  // Only when there are any: a build that bundled no sounds should not
  // show a heading promising some.
  if (builtIn.length > 0) {
    groups.push({ title: 'Built in', removable: false, builtIn: true, files: builtIn });
  }
  groups.push({ title: 'Internal', removable: false, builtIn: false, files: internal });
  const byVolume = new Map<string, SoundfontInfo[]>();
  for (const file of files.filter((f) => f.origin === 'removable')) {
    const key = file.volume ?? 'Removable drive';
    byVolume.set(key, [...(byVolume.get(key) ?? []), file]);
  }
  for (const [title, group] of [...byVolume].sort(([a], [b]) => a.localeCompare(b))) {
    groups.push({ title, removable: true, builtIn: false, files: group });
  }
  return groups;
}

/**
 * What a new instrument should load, when the box has anything to offer.
 *
 * The smallest built-in, because size is memory here: a bundled General
 * MIDI bank can be 206 MB resident, and a new instrument silently costing
 * that on a 2 GB Pi is a bad way to meet the feature. The user can pick any
 * of them a moment later; this only decides what plays first.
 *
 * Falls back to the smallest uploaded font so a build that bundles nothing
 * still starts somewhere, and to `undefined` when the library is empty —
 * a voiceless instrument is valid, and the console explains it.
 */
export function defaultSoundfont(files: readonly SoundfontInfo[]): string | undefined {
  const smallest = (origin: SoundfontInfo['origin']) =>
    files
      .filter((f) => f.origin === origin)
      .sort((a, b) => a.bytes - b.bytes)[0]?.id;
  return smallest('built_in') ?? smallest('internal');
}

/** Why this file cannot be deleted from the console, or null. */
export function deleteBlocked(
  file: SoundfontInfo,
  usedBy: readonly string[],
  recording: boolean,
): string | null {
  if (file.origin === 'built_in') {
    return 'built in — part of this installation';
  }
  if (file.origin === 'removable') {
    return `lives on “${file.volume ?? 'a drive'}” — remove it there`;
  }
  if (recording) return 'stop recording first';
  if (usedBy.length > 0) return `in use by ${usedBy.join(', ')}`;
  return null;
}

/**
 * Two empty states, never merged: nothing loaded at all is a different
 * situation from a drive that holds no soundfonts, and each has a different
 * next step.
 */
export function emptyState(files: readonly SoundfontInfo[], drivesPresent: boolean): string {
  if (files.length > 0) return '';
  return drivesPresent
    ? 'A drive is connected, but it holds no .sf2 files.'
    : 'No soundfonts on this appliance yet — upload one below.';
}

/**
 * Refuse before spending the transfer. Returns a sentence or null.
 *
 * Both numbers appear in the too-big message: "too big" alone leaves you
 * guessing how much smaller is small enough.
 */
export function validateUpload(
  name: string,
  bytes: number,
  maxBytes: number,
): string | null {
  if (!name.toLowerCase().endsWith(SOUNDFONT_EXT)) {
    return 'that is not a .sf2 file';
  }
  if (bytes > maxBytes) {
    return `${formatBytes(bytes)} is over this appliance’s ${formatBytes(maxBytes)} limit`;
  }
  if (bytes === 0) return 'that file is empty';
  return null;
}

/** The console's dialect for a refused upload. */
export function uploadErrorMessage(status: number, detail?: string): string {
  switch (status) {
    case 0:
      return 'the connection dropped — nothing was added';
    case 409:
      return detail ?? 'locked while recording';
    case 413:
      return detail ?? 'too big for this appliance';
    case 422:
      return detail ?? 'that is not a SoundFont this daemon can read';
    default:
      return detail ?? 'the upload failed';
  }
}

/**
 * Honest progress: bytes handed to the socket are not bytes committed, so
 * once the last one is sent the phase changes rather than sitting at 100%.
 */
export function progressLine(sent: number, total: number): string {
  if (sent >= total) return 'Checking the file…';
  return `Uploading… ${formatBytes(sent)} of ${formatBytes(total)}`;
}
