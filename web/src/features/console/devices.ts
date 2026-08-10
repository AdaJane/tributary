/**
 * Patchbay presentation: the daemon's device document grouped into the
 * sections the modal draws. ALSA routing aliases fold into one "System
 * default input" section (patched as `device: null`); every other device
 * gets its own lettered stage box.
 */
import type { DeviceReport } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import { INPUT_CHANNELS, deviceLetters } from './linked-inputs';

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
  /** The stored project name this device was matched from (renamed
   * hardware adopted by reconciliation). */
  reconciledFrom: string | null;
}

const ROUTING_ALIASES = new Set(['default', 'sysdefault', 'pipewire', 'pulse', 'jack']);

export type SourceKind = 'default' | 'mic' | 'webcam' | 'usb' | 'line';

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

function patchedMax(strips: readonly StripState[], device: string | null): number {
  return strips.reduce(
    (max, s) =>
      s.input && (s.input.device ?? null) === device
        ? Math.max(max, s.input.device_channel)
        : max,
    -1,
  );
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

  const defaultChannels = active?.channels ?? INPUT_CHANNELS;
  const sections: PatchbaySection[] = [
    {
      device: null,
      title: 'System default input',
      sublabel: active ? (active.label ?? active.name) : null,
      letter: 'A',
      channels: defaultChannels,
      jackCount: Math.max(defaultChannels, patchedMax(strips, null) + 1),
      status: active ? (active.status as SectionStatus) : 'absent',
      reconciledFrom: null,
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
      reconciledFrom: device.reconciled_from ?? null,
    });
  }
  return sections;
}
