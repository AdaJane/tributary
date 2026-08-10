import { describe, expect, it } from 'vitest';

import type { StripState } from '../../ws/messages';
import {
  deviceLetters,
  jackKey,
  jackLabel,
  sharedInputGroups,
  stripeColor,
} from './linked-inputs';

const strip = (id: number, device: string | null, channel: number | null): StripState =>
  ({
    id,
    name: `Ch ${id + 1}`,
    input:
      channel === null
        ? null
        : { device: device ?? undefined, device_channel: channel },
  }) as StripState;

const letters = deviceLetters(['dock', 'interface']);

describe('sharedInputGroups', () => {
  it('links only jacks feeding two or more strips', () => {
    const links = sharedInputGroups(
      [strip(0, null, 0), strip(1, null, 0), strip(2, null, 3)],
      letters,
    );
    expect(links.size).toBe(1);
    expect(links.get(jackKey(null, 0))?.stripIds).toEqual([0, 1]);
  });

  it('the same channel on two devices is two different jacks', () => {
    const links = sharedInputGroups(
      [
        strip(0, null, 0),
        strip(1, null, 0),
        strip(2, 'dock', 0),
        strip(3, 'dock', 0),
      ],
      letters,
    );
    expect(links.size).toBe(2);
    const a = links.get(jackKey(null, 0));
    const b = links.get(jackKey('dock', 0));
    expect(a?.stripIds).toEqual([0, 1]);
    expect(b?.stripIds).toEqual([2, 3]);
    expect(a?.label).toBe('IN 1');
    expect(b?.label).toBe('B1');
  });

  it('unpatched strips never link', () => {
    const links = sharedInputGroups([strip(0, null, null), strip(1, null, null)], letters);
    expect(links.size).toBe(0);
  });
});

describe('jack identity', () => {
  it('colors are keyed by jack identity, stable across membership', () => {
    expect(stripeColor(jackKey(null, 2))).toBe(stripeColor(jackKey(null, 2)));
    // Not universally distinct (six colors), but the classic collision —
    // channel 0 on two devices — must not share a KEY.
    expect(jackKey(null, 0)).not.toBe(jackKey('dock', 0));
  });

  it('letters: A is always the default box, then names in stable order', () => {
    const map = deviceLetters(['zoom', 'dock']);
    expect(map.get(null)).toBe('A');
    expect(map.get('dock')).toBe('B');
    expect(map.get('zoom')).toBe('C');
    // Listing order doesn't matter — only the name set does.
    const shuffled = deviceLetters(['dock', 'zoom']);
    expect(shuffled.get('zoom')).toBe('C');
  });

  it('labels print console-style', () => {
    expect(jackLabel(null, 0, letters)).toBe('IN 1');
    expect(jackLabel(null, 7, letters)).toBe('IN 8');
    expect(jackLabel('dock', 1, letters)).toBe('B2');
    expect(jackLabel('unknown', 0, letters)).toBe('?1');
  });
});
