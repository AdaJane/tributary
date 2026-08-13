import { describe, expect, it } from 'vitest';

import type { DeviceReport } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import { groupPatchbay, sourceKind } from './devices';

const dev = (
  name: string,
  overrides: Partial<DeviceReport> = {},
): DeviceReport => ({
  name,
  channels: 2,
  active: false,
  status: 'available',
  patched: false,
  underruns: 0,
  overruns: 0,
  profiles: [],
  reconciled_from: null,
  ...overrides,
});

/** A device on a card offering a real profile choice. */
const carded = (name: string, overrides: Partial<DeviceReport> = {}): DeviceReport =>
  dev(name, {
    card: 'alsa_card.umc',
    profile: 'input:analog-stereo',
    profiles: [
      { name: 'input:analog-stereo', description: 'Analog Stereo Input' },
      { name: 'pro-audio', description: 'Pro Audio' },
    ],
    ...overrides,
  });

const strip = (id: number, device: string | null, channel: number): StripState =>
  ({
    id,
    name: `Ch ${id + 1}`,
    input: { device: device ?? undefined, device_channel: channel },
  }) as StripState;

describe('groupPatchbay', () => {
  it('folds the active alias into the default section and letters the rest', () => {
    const sections = groupPatchbay(
      [
        dev('default', { active: true, status: 'open', patched: true }),
        dev('pipewire'),
        dev('sof-hda-dsp'),
        dev('ThinkPad Thunderbolt 4 Dock USB'),
      ],
      [],
    );
    expect(sections.map((s) => [s.letter, s.title])).toEqual([
      ['A', 'System default input'],
      ['B', 'ThinkPad Thunderbolt 4 Dock USB'],
      ['C', 'sof-hda-dsp'],
    ]);
    expect(sections[0].status).toBe('open');
    expect(sections[0].channels).toBe(2);
  });

  it('carries the open-failure reason onto every section the device feeds', () => {
    const sections = groupPatchbay(
      [
        dev('alsa_input.usb-UMC1820.multichannel-input', {
          label: 'UMC1820 Multichannel',
          active: true,
          status: 'failed',
          patched: true,
          error: 'audio backend not running',
        }),
      ],
      [],
    );
    // The default box follows the same failed source: both explain why.
    expect(sections.map((s) => [s.letter, s.status, s.error])).toEqual([
      ['A', 'failed', 'audio backend not running'],
      ['B', 'failed', 'audio backend not running'],
    ]);
  });

  it('an absent project device still forms a section with its dead patches', () => {
    const sections = groupPatchbay(
      [
        dev('default', { active: true }),
        dev('Scarlett 18i20 USB', { status: 'absent', patched: true, channels: 0 }),
      ],
      [strip(0, 'Scarlett 18i20 USB', 5)],
    );
    const scarlett = sections.find((s) => s.device === 'Scarlett 18i20 USB');
    expect(scarlett?.status).toBe('absent');
    expect(scarlett?.jackCount).toBe(6);
  });

  it('pulse sources print their friendly labels, and the default source also gets its own box', () => {
    const sections = groupPatchbay(
      [
        dev('alsa_input.usb-046d_C922.analog-stereo', {
          label: 'C922 Pro Stream Webcam Analog Stereo',
          active: true,
          status: 'open',
        }),
        dev('alsa_input.pci.mic6', {
          label: 'Meteor Lake-P HD Audio Controller Digital Microphone',
        }),
        dev('alsa_input.usb-Dock.mono-fallback', {
          label: 'ThinkPad Thunderbolt 4 Dock USB Audio Mono',
          channels: 1,
        }),
      ],
      [],
    );
    expect(sections.map((s) => [s.letter, s.title])).toEqual([
      ['A', 'System default input'],
      ['B', 'Meteor Lake-P HD Audio Controller Digital Microphone'],
      ['C', 'C922 Pro Stream Webcam Analog Stereo'],
      ['D', 'ThinkPad Thunderbolt 4 Dock USB Audio Mono'],
    ]);
    expect(sections[0].sublabel).toBe('C922 Pro Stream Webcam Analog Stereo');
    expect(sections[2].device).toBe('alsa_input.usb-046d_C922.analog-stereo');
    expect(sections[3].channels).toBe(1);
  });

  it('a reconciled device carries its old name', () => {
    const sections = groupPatchbay(
      [
        dev('default', { active: true }),
        dev('Dock USB #2', { status: 'open', reconciled_from: 'Dock USB' }),
      ],
      [],
    );
    expect(sections[1].reconciledFrom).toBe('Dock USB');
  });

  it('with no enumeration the default box draws nothing it cannot prove', () => {
    const sections = groupPatchbay([], []);
    expect(sections).toHaveLength(1);
    expect(sections[0].jackCount).toBe(0);
    expect(sections[0].status).toBe('absent');
  });

  it('keeps a dead patch visible even with no enumeration behind it', () => {
    const sections = groupPatchbay([], [strip(0, null, 4)]);
    expect(sections[0].jackCount).toBe(5);
    expect(sections[0].channels).toBe(0);
  });

  it('offers a profile switch only where there is a real choice to make', () => {
    const sections = groupPatchbay(
      [
        carded('umc'),
        dev('webcam', { card: 'alsa_card.webcam', profile: 'x', profiles: [] }),
        dev('bare'),
      ],
      [],
    );
    const byTitle = new Map(sections.map((s) => [s.title, s]));
    expect(byTitle.get('umc')?.profiles).toHaveLength(2);
    expect(byTitle.get('umc')?.card).toBe('alsa_card.umc');
    expect(byTitle.get('webcam')?.profiles).toEqual([]);
    expect(byTitle.get('bare')?.profiles).toEqual([]);
  });

  it('hides the switch when the active profile is not among the options', () => {
    // A picker that cannot represent the current state would misreport it.
    const sections = groupPatchbay([carded('umc', { profile: 'a2dp-sink' })], []);
    expect(sections[1].profiles).toEqual([]);
  });

  it('never offers a profile switch on the follower box', () => {
    const sections = groupPatchbay([carded('umc', { active: true })], []);
    expect(sections[0].title).toBe('System default input');
    expect(sections[0].profiles).toEqual([]);
    expect(sections[1].profiles).toHaveLength(2);
  });
});

describe('sourceKind', () => {
  it('reads the glyph off the print', () => {
    expect(sourceKind(null, 'System default input')).toBe('default');
    expect(sourceKind('x', 'C922 Pro Stream Webcam Analog Stereo')).toBe('webcam');
    expect(sourceKind('x', 'Meteor Lake-P Digital Microphone')).toBe('mic');
    expect(sourceKind('x', 'ThinkPad Thunderbolt 4 Dock USB Audio Mono')).toBe('usb');
    expect(sourceKind('x', 'Scarlett 18i20 3rd Gen')).toBe('line');
  });
});
