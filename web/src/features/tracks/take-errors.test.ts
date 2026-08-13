import { describe, expect, it } from 'vitest';

import { deleteErrorMessage, peaksErrorMessage, selectErrorMessage } from './take-errors';

describe('take error messages', () => {
  it('says what happened, not what the status code was', () => {
    expect(selectErrorMessage(404)).toMatch(/no longer there/);
    expect(deleteErrorMessage(404)).toMatch(/already gone/);
    expect(peaksErrorMessage(404)).toMatch(/no longer on the drive/);
  });

  it('prefers the daemon’s own sentence when it sent one', () => {
    expect(selectErrorMessage(409, 'no take switching while recording')).toBe(
      'no take switching while recording',
    );
    expect(deleteErrorMessage(409, "stop playback before deleting the take you're playing")).toBe(
      "stop playback before deleting the take you're playing",
    );
  });

  /** The bug these exist for: a failed load used to render as nothing. */
  it('never returns an empty string for any failure', () => {
    for (const status of [404, 409, 422, 500, 'network' as const]) {
      expect(selectErrorMessage(status).length).toBeGreaterThan(0);
      expect(deleteErrorMessage(status).length).toBeGreaterThan(0);
    }
    for (const status of [404, 500, 'network' as const, 'malformed' as const]) {
      expect(peaksErrorMessage(status).length).toBeGreaterThan(0);
    }
  });
});
