import { describe, expect, it } from 'vitest';

import {
  MAX_SESSION_NAME,
  SEED_LABELS,
  deleteBlocked,
  openBlocked,
  seedConsequence,
  seedNeedsConfirm,
  sessionErrorMessage,
  sortedSessions,
  takeCountLabel,
  validateSessionName,
} from './session-logic';
import type { SessionLike } from './session-logic';

function session(over: Partial<SessionLike> = {}): SessionLike {
  return {
    id: '1786380405-gig',
    name: 'Gig',
    createdAtUnix: 1_786_380_405,
    takeCount: 3,
    open: false,
    ...over,
  };
}

describe('validateSessionName', () => {
  it('accepts ordinary names, trimming the edges', () => {
    expect(validateSessionName('Friday Night')).toBeNull();
    expect(validateSessionName('  Gig  ')).toBeNull();
  });

  it('mirrors the daemon limit so the button greys out before a round trip', () => {
    expect(validateSessionName('x'.repeat(MAX_SESSION_NAME))).toBeNull();
    expect(validateSessionName('x'.repeat(MAX_SESSION_NAME + 1))).toBe('at most 64 characters');
  });

  it('refuses empty and multi-line names', () => {
    expect(validateSessionName('')).toBe('name the session');
    expect(validateSessionName('   ')).toBe('name the session');
    expect(validateSessionName('two\nlines')).toBe('one line, no control characters');
  });
});

describe('deleteBlocked / openBlocked', () => {
  /** The daemon refuses this too; the UI just says why up front rather
   *  than failing on tap. */
  it('refuses deleting the open session, with a reason', () => {
    expect(deleteBlocked(session({ open: true }), false)).toBe('open another session first');
    expect(deleteBlocked(session(), false)).toBeNull();
  });

  it('refuses both while recording', () => {
    expect(deleteBlocked(session(), true)).toBe('stop recording first');
    expect(openBlocked(session(), true)).toBe('stop recording first');
  });

  it('says an already-open session is already open', () => {
    expect(openBlocked(session({ open: true }), false)).toBe('already open');
  });
});

describe('takeCountLabel', () => {
  it('distinguishes empty, one, and many', () => {
    expect(takeCountLabel(0)).toBe('empty');
    expect(takeCountLabel(1)).toBe('1 take');
    expect(takeCountLabel(12)).toBe('12 takes');
  });
});

describe('seedConsequence', () => {
  /** The label says what you get; this says what you lose. */
  it('names what resets and which session keeps the desk', () => {
    expect(seedConsequence('mapping', 8, 'Friday Night')).toBe(
      '8 channels keep their inputs and arming; EQ, sends and levels reset. “Friday Night” keeps its own copy.',
    );
    expect(seedConsequence('template', 8, 'Friday Night')).toMatch(/one channel on input 1/);
    expect(seedConsequence('template', 8, 'Friday Night')).toMatch(/Friday Night/);
  });

  it('pluralises a single channel', () => {
    expect(seedConsequence('mapping', 1, 'Gig')).toMatch(/^1 channel keep/);
  });

  it('promises no change when the desk is copied', () => {
    expect(seedConsequence('console', 8, 'Gig')).toMatch(/nothing changes/);
  });

  /** A confirmation that fires when nothing will happen is the kind that
   *  gets tapped through without reading. */
  it('only asks for confirmation when the desk actually changes', () => {
    expect(seedNeedsConfirm('template')).toBe(true);
    expect(seedNeedsConfirm('mapping')).toBe(true);
    expect(seedNeedsConfirm('console')).toBe(false);
  });

  it('labels every seed', () => {
    expect(Object.values(SEED_LABELS).every((l) => l.length > 0)).toBe(true);
  });
});

describe('sortedSessions', () => {
  it('puts the newest first regardless of wire order', () => {
    const rows = sortedSessions([
      { id: 'a', createdAtUnix: 100 },
      { id: 'c', createdAtUnix: 300 },
      { id: 'b', createdAtUnix: 200 },
    ]);
    expect(rows.map((r) => r.id)).toEqual(['c', 'b', 'a']);
  });
});

describe('sessionErrorMessage', () => {
  it('maps each refusal to something actionable', () => {
    expect(sessionErrorMessage(404)).toMatch(/no longer there/);
    expect(sessionErrorMessage(409, 'stop recording before deleting a session')).toBe(
      'stop recording before deleting a session',
    );
    expect(sessionErrorMessage('network')).toBe('daemon unreachable');
  });
});
