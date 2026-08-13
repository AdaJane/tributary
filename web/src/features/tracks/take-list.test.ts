import { describe, expect, it } from 'vitest';

import {
  kHz,
  sortedTakes,
  takeDuration,
  takeLabel,
  takePlayability,
  takeTime,
  trackCountLabel,
} from './take-list';

describe('takeLabel', () => {
  it('pads so a column of takes stays aligned', () => {
    expect(takeLabel(3)).toBe('T03');
    expect(takeLabel(12)).toBe('T12');
    expect(takeLabel(100)).toBe('T100');
  });
});

describe('takeDuration', () => {
  it('reads as minutes:seconds, growing an hours field only when needed', () => {
    expect(takeDuration(187)).toBe('3:07');
    expect(takeDuration(0)).toBe('0:00');
    expect(takeDuration(59.9)).toBe('0:59');
    expect(takeDuration(60)).toBe('1:00');
    expect(takeDuration(3731)).toBe('1:02:11');
  });

  it('never renders a negative or NaN-looking duration', () => {
    expect(takeDuration(-5)).toBe('0:00');
  });
});

describe('takeTime', () => {
  /** Pinned locale and zone: otherwise this asserts differently on every
   *  machine, which is why the formatter is injected. */
  it('prints the wall-clock start time', () => {
    const fmt = new Intl.DateTimeFormat('en-GB', {
      hour: '2-digit',
      minute: '2-digit',
      timeZone: 'UTC',
    });
    expect(takeTime(1_786_380_405, fmt)).toBe('16:46');
  });
});

describe('trackCountLabel', () => {
  it('pluralises', () => {
    expect(trackCountLabel(1)).toBe('1 track');
    expect(trackCountLabel(10)).toBe('10 tracks');
  });
});

describe('takePlayability', () => {
  it('passes a matching rate', () => {
    expect(takePlayability(48_000, 48_000)).toEqual({ playable: true, reason: null });
  });

  /** The take is still selectable and viewable — only PLAY is blocked,
   *  and the console prints why instead of surfacing a 422. */
  it('explains a rate mismatch with both rates', () => {
    const { playable, reason } = takePlayability(44_100, 48_000);
    expect(playable).toBe(false);
    expect(reason).toBe('cut at 44.1 kHz · the engine runs 48 kHz');
  });
});

describe('kHz', () => {
  it('drops trailing zeros but keeps a real fraction', () => {
    expect(kHz(48_000)).toBe('48 kHz');
    expect(kHz(96_000)).toBe('96 kHz');
    expect(kHz(44_100)).toBe('44.1 kHz');
  });
});

describe('sortedTakes', () => {
  it('puts the newest first whatever the wire said', () => {
    expect(sortedTakes([{ take: 2 }, { take: 10 }, { take: 1 }]).map((t) => t.take)).toEqual([
      10, 2, 1,
    ]);
  });
});
