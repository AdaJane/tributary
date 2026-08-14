/**
 * Grouping and explaining the output bay. Pure — the store maps wire
 * shapes, this file only decides shape, order and wording, the same split
 * `groupPatchbay` uses for input devices.
 */

import type { OutputDeviceReport, OutputPatch } from './output-patch';
import { outputKey } from './output-patch';

export type OutputSectionStatus = 'open' | 'available' | 'failed' | 'absent';

export interface OutputSection {
  /** `null` = the system default output. */
  device: string | null;
  title: string;
  /** Total channels, including any the monitor has taken. */
  channels: number;
  /** Channels the monitor took and the patch bay may not offer. */
  monitorChannels: number;
  /** Jacks to draw: the patchable channels, plus any a patch still names. */
  jackCount: number;
  status: OutputSectionStatus;
  error: string | null;
  muted: boolean;
  volumePercent: number | null;
  /** A device a patch names that nothing enumerated. */
  ghost: boolean;
}

const STATUS: Record<OutputDeviceReport['status'], OutputSectionStatus> = {
  open: 'open',
  available: 'available',
  failed: 'failed',
  absent: 'absent',
};

/** Highest channel any patch on `device` names, or -1. */
function patchedMax(patches: readonly OutputPatch[], device: string | null): number {
  return patches.reduce(
    (max, p) => ((p.device ?? null) === device ? Math.max(max, p.channel) : max),
    -1,
  );
}

/**
 * One section per output device, hardware first, ghosts last.
 *
 * A device a patch still names but nothing enumerated gets a GHOST section
 * — the direct mirror of the instrument ghost on the input side, and the
 * important half: without it a strip's OUT button prints `OUT 3` with
 * nothing anywhere in the room to explain why nothing comes out of it.
 */
export function groupOutputBay(
  devices: readonly OutputDeviceReport[],
  patches: readonly OutputPatch[],
): OutputSection[] {
  const sections: OutputSection[] = devices.map((device) => {
    const key = device.active ? null : device.name;
    const patchable = Math.max(0, device.channels - device.monitor_channels);
    return {
      device: key,
      title: device.label ?? device.name,
      channels: device.channels,
      monitorChannels: device.monitor_channels,
      // A patch past the device's width keeps its socket, dimmed. A silent
      // output must be explainable from the patch bay.
      jackCount: Math.max(patchable, patchedMax(patches, key) + 1),
      status: STATUS[device.status],
      error: device.error ?? null,
      muted: device.muted,
      volumePercent: device.volume_percent ?? null,
      ghost: false,
    };
  });

  const known = new Set(sections.map((s) => s.device));
  const orphans = new Set<string | null>();
  for (const patch of patches) {
    const device = patch.device ?? null;
    if (!known.has(device)) orphans.add(device);
  }
  for (const device of [...orphans].sort((a, b) => String(a).localeCompare(String(b)))) {
    sections.push({
      device,
      title: device ?? 'System default output',
      channels: 0,
      monitorChannels: 0,
      jackCount: patchedMax(patches, device) + 1,
      status: 'absent',
      error: null,
      muted: false,
      volumePercent: null,
      ghost: true,
    });
  }
  return sections;
}

/** Below this, a device is turned down far enough to be inaudible. */
const SILENCED_BELOW_PERCENT = 10;

/**
 * Why a healthy-looking output puts nothing in the room.
 *
 * More load-bearing than its input-side twin: a muted INPUT at least shows
 * a dead meter, and an output has no meter at all — so if this sentence is
 * not here, a silent PA has no explanation anywhere in the console.
 */
export function outputSilencedReason(section: OutputSection): string | null {
  if (section.muted) return 'muted in the system mixer';
  const volume = section.volumePercent;
  // Unknown is not zero: a backend that reports no volume must not be
  // accused of silencing anything.
  if (volume !== null && volume < SILENCED_BELOW_PERCENT) {
    return `system output volume at ${volume}%`;
  }
  return null;
}

/** What a ghost section says instead of a device error. */
export function ghostReason(section: OutputSection): string | null {
  return section.ghost
    ? 'this output is not connected — patch these channels somewhere else'
    : null;
}

/** The print on one output jack: `OUT 3`, numbered as the hardware is. */
export function jackLabel(section: OutputSection, channel: number): string {
  return `OUT ${channel + section.monitorChannels + 1}`;
}

/** A jack past what the device actually has. */
export function jackIsDead(section: OutputSection, channel: number): boolean {
  return channel >= Math.max(0, section.channels - section.monitorChannels);
}

/** Stable key for a section's jack. */
export function jackKey(section: OutputSection, channel: number): string {
  return outputKey(section.device, channel);
}
