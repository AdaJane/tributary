/**
 * Patchbay presentation: the daemon's device document grouped into the
 * sections the modal draws. ALSA routing aliases fold into one "System
 * default input" section (patched as `device: null`); every other device
 * gets its own lettered stage box.
 */
import type { DeviceReport } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import { inputSource } from './input-source';
import { deviceLetters } from './linked-inputs';

export type SectionStatus = 'open' | 'available' | 'failed' | 'absent';

export interface PatchbaySection {
  /** Patch identity: null = the system default input. */
  device: string | null;
  title: string;
  /** Secondary print — the default box names the source it follows. */
  sublabel: string | null;
  letter: string;
  channels: number;
  /** Tiles to draw: every channel, plus any jack a strip is patched to
   * past the device (a dead patch must stay visible). */
  jackCount: number;
  status: SectionStatus;
  /** The daemon's open-failure reason, when it knows one. */
  error: string | null;
  /** The stored project name this device was matched from (renamed
   * hardware adopted by reconciliation). */
  reconciledFrom: string | null;
  /** The sound card to address a profile change to. */
  card: string | null;
  /** The card's active profile. */
  profile: string | null;
  /** Profiles worth offering — empty unless there is a real choice, in
   * which case `channels` above is a setting rather than a hardware limit. */
  profiles: DeviceReport['profiles'];
  /** The source layer has this input muted. */
  muted: boolean;
  /** The quietest channel's volume as a percentage of unity, or null when
   * the source layer never said (raw ALSA, absent devices). */
  volumePercent: number | null;
}

const ROUTING_ALIASES = new Set(['default', 'sysdefault', 'pipewire', 'pulse', 'jack']);

/** The ALSA profile that exposes every channel a card has. The server
 * publishes no per-profile channel count, so this name is the only honest
 * hint we can give about which profile reveals the missing inputs. */
export const PRO_AUDIO_PROFILE = 'pro-audio';

/** Below this, an input is not "quiet" — it is off in all but name. A
 * source at 5 % is 26 dB down: nothing the strip's gain is meant to
 * rescue, and worth saying out loud. */
const SILENCED_BELOW_PERCENT = 10;

/** Why this input will read silent no matter what the console does, or
 * null when nothing in the source layer is holding it down.
 *
 * This is the one fault the rest of the report cannot express: a muted or
 * near-zero source enumerates, opens, streams and reports `status: open`
 * with `error: null`, feeding silence to every meter behind it. */
export function silencedReason(section: PatchbaySection): string | null {
  if (section.muted) return 'muted in the system mixer';
  const volume = section.volumePercent;
  // Unknown is not zero: a backend that reports no volume must not be
  // accused of silencing anything.
  if (volume !== null && volume < SILENCED_BELOW_PERCENT) {
    return `system input volume at ${volume}%`;
  }
  return null;
}

export type SourceKind = 'default' | 'mic' | 'webcam' | 'usb' | 'line' | 'instrument';

/** Guess a glyph from the source's print — supplementary only (the title
 * stays the identity), so a wrong guess costs nothing but style. */
export function sourceKind(device: string | null, title: string): SourceKind {
  if (device === null) return 'default';
  const print = title.toLowerCase();
  if (/webcam|camera/.test(print)) return 'webcam';
  if (/\bmic\b|microphone/.test(print)) return 'mic';
  if (/dock|usb/.test(print)) return 'usb';
  return 'line';
}

function switchable(device: DeviceReport): boolean {
  return (
    device.card !== null &&
    device.card !== undefined &&
    device.profiles.length > 1 &&
    device.profiles.some((p) => p.name === device.profile)
  );
}

function patchedMax(strips: readonly StripState[], device: string | null): number {
  return strips.reduce((max, s) => {
    const source = inputSource(s.input);
    return source?.kind === 'device' && source.device === device
      ? Math.max(max, source.channel)
      : max;
  }, -1);
}

/** Group the device document into modal sections: the default box first
 * (always present — patches may point at it even with no enumeration),
 * then EVERY source as its own lettered box, mirroring the system sound
 * menu. The default's source also gets a box of its own: A follows the
 * system; the named box pins it. */
export function groupPatchbay(
  devices: readonly DeviceReport[],
  strips: readonly StripState[],
): PatchbaySection[] {
  const active = devices.find((d) => d.active);
  // Plain code-unit sort, matching deviceLetters' ordering exactly.
  const named = devices
    .filter((d) => !ROUTING_ALIASES.has(d.name))
    .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  const letters = deviceLetters(named.map((d) => d.name));

  // No enumerated default = no jacks, not a guessed handful. Inventing a
  // count here drew phantom sockets that looked exactly like a real device
  // and hid the fact that enumeration had found nothing.
  const defaultChannels = active?.channels ?? 0;
  const sections: PatchbaySection[] = [
    {
      device: null,
      title: 'System default input',
      sublabel: active ? (active.label ?? active.name) : null,
      letter: 'A',
      channels: defaultChannels,
      jackCount: Math.max(defaultChannels, patchedMax(strips, null) + 1),
      status: active ? (active.status as SectionStatus) : 'absent',
      error: active?.error ?? null,
      reconciledFrom: null,
      card: active?.card ?? null,
      profile: active?.profile ?? null,
      // Section A follows whatever the system default is; the same device
      // also gets its own lettered box, and that is where it is configured.
      profiles: [],
      // Mute DOES follow, unlike the profile: A is the box a default patch
      // is drawn in, so the reason its jacks are silent belongs here too.
      muted: active?.muted ?? false,
      volumePercent: active?.volume_percent ?? null,
    },
  ];
  for (const device of named) {
    sections.push({
      device: device.name,
      title: device.label ?? device.name,
      sublabel: null,
      letter: letters.get(device.name) ?? '?',
      channels: device.channels,
      jackCount: Math.max(device.channels, patchedMax(strips, device.name) + 1),
      status: device.status as SectionStatus,
      error: device.error ?? null,
      reconciledFrom: device.reconciled_from ?? null,
      card: device.card ?? null,
      profile: device.profile ?? null,
      // Offer the choice only when there IS one, and only when the active
      // profile is among the options — a picker that cannot show the
      // current state would misreport it.
      profiles: switchable(device) ? device.profiles : [],
      muted: device.muted,
      volumePercent: device.volume_percent ?? null,
    });
  }
  return sections;
}
