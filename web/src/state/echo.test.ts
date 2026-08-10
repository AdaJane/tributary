import { describe, expect, it } from 'vitest';

import {
  EMPTY_HOT,
  HOT_LINGER_MS,
  markCaptured,
  markSent,
  shouldAcceptEcho,
} from './echo';

const ME = 'tab-1';
const OTHER = 'tab-2';

describe('shouldAcceptEcho', () => {
  it('accepts everything when nothing is hot', () => {
    expect(shouldAcceptEcho(EMPTY_HOT, 'fader:0', OTHER, ME, 1000)).toBe(true);
    expect(shouldAcceptEcho(EMPTY_HOT, 'fader:0', undefined, ME, 1000)).toBe(true);
  });

  it('drops foreign echoes while the param is captured', () => {
    const hot = markCaptured(EMPTY_HOT, 'fader:0');
    expect(shouldAcceptEcho(hot, 'fader:0', OTHER, ME, 1000)).toBe(false);
    expect(shouldAcceptEcho(hot, 'fader:0', undefined, ME, 1000)).toBe(false);
  });

  it('always accepts our own acks, even while hot', () => {
    const hot = markCaptured(EMPTY_HOT, 'fader:0');
    expect(shouldAcceptEcho(hot, 'fader:0', ME, ME, 1000)).toBe(true);
  });

  it('other params stay unaffected by a hot one', () => {
    const hot = markCaptured(EMPTY_HOT, 'fader:0');
    expect(shouldAcceptEcho(hot, 'pan:0', OTHER, ME, 1000)).toBe(true);
  });

  it('the hot window lingers after the last send, then releases', () => {
    const hot = markSent(EMPTY_HOT, 'fader:0', 1000);
    expect(shouldAcceptEcho(hot, 'fader:0', OTHER, ME, 1000 + HOT_LINGER_MS - 1)).toBe(false);
    expect(shouldAcceptEcho(hot, 'fader:0', OTHER, ME, 1000 + HOT_LINGER_MS)).toBe(true);
  });
});
