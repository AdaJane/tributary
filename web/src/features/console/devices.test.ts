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
  reconciled_from: null,
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

  it('with no enumeration the default box still draws the classic eight', () => {
    const sections = groupPatchbay([], [strip(0, null, 0)]);
    expect(sections).toHaveLength(1);
    expect(sections[0].jackCount).toBe(8);
    expect(sections[0].status).toBe('absent');
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
