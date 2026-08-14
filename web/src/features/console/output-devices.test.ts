import { describe, expect, it } from 'vitest';

import {
  ghostReason,
  groupOutputBay,
  jackIsDead,
  jackLabel,
  outputSilencedReason,
} from './output-devices';
import type { OutputDeviceReport, OutputPatch } from './output-patch';

function device(over: Partial<OutputDeviceReport> = {}): OutputDeviceReport {
  return {
    name: 'interface',
    label: null,
    channels: 8,
    monitor_channels: 0,
    active: false,
    status: 'open',
    patched: true,
    underruns: 0,
    overruns: 0,
    xruns: 0,
    worst_block_us: 0,
    muted: false,
    volume_percent: null,
    error: null,
    ...over,
  } as OutputDeviceReport;
}

function patch(device: string | null, channel: number): OutputPatch {
  return {
    ...(device === null ? {} : { device }),
    channel,
    source_channel: 0,
    tap: 'pre_fader',
    source: { kind: 'master' },
  } as OutputPatch;
}

describe('groupOutputBay', () => {
  it('an unplugged output a patch still names gets a ghost section', () => {
    // The bug this exists for: without the ghost, the strip's OUT button
    // prints OUT 4 and there is nothing anywhere in the room that explains
    // why nothing comes out of it — the exact failure the instrument ghost
    // was added to prevent, one room over.
    const sections = groupOutputBay([], [patch('unplugged', 3)]);
    expect(sections).toHaveLength(1);
    expect(sections[0].ghost).toBe(true);
    expect(sections[0].status).toBe('absent');
    expect(sections[0].jackCount).toBe(4);
    expect(ghostReason(sections[0])).toContain('not connected');
  });

  it('a patch past the device keeps its socket rather than vanishing', () => {
    const sections = groupOutputBay([device({ channels: 2 })], [patch('interface', 5)]);
    expect(sections[0].jackCount).toBe(6);
    expect(jackIsDead(sections[0], 5)).toBe(true);
    expect(jackIsDead(sections[0], 1)).toBe(false);
  });

  it('the monitor channels are never offered as jacks but still count in the print', () => {
    // The real-time backend shares one card between the control-room feed
    // and everything else, so OUT 1 on an 8-channel card with 2 taken is
    // the card's physical output 3.
    const sections = groupOutputBay([device({ channels: 8, monitor_channels: 2 })], []);
    expect(sections[0].jackCount).toBe(6);
    expect(jackLabel(sections[0], 0)).toBe('OUT 3');
    expect(jackLabel(sections[0], 5)).toBe('OUT 8');
  });

  it('the default device is keyed null so a patch without a device finds it', () => {
    const sections = groupOutputBay([device({ active: true })], [patch(null, 1)]);
    expect(sections[0].device).toBeNull();
    expect(sections[0].ghost).toBe(false);
  });

  it('a muted output says so without calling itself failed', () => {
    // An output has no meter. If this sentence is missing, a dead PA has
    // no explanation anywhere in the console.
    const sections = groupOutputBay([device({ muted: true })], []);
    expect(sections[0].status).toBe('open');
    expect(outputSilencedReason(sections[0])).toBe('muted in the system mixer');
  });

  it('an unknown volume is not an accusation of silence', () => {
    const sections = groupOutputBay([device({ volume_percent: null })], []);
    expect(outputSilencedReason(sections[0])).toBeNull();
    const quiet = groupOutputBay([device({ volume_percent: 4 })], []);
    expect(outputSilencedReason(quiet[0])).toContain('4%');
  });
});
