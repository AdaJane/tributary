import { describe, expect, it } from 'vitest';
import { uuidv4 } from './uuid';

const V4_SHAPE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** A Crypto stand-in for insecure contexts: getRandomValues only —
 * randomUUID is absent on plain-HTTP origins like http://tributary.local. */
const insecureCrypto = {
  getRandomValues: crypto.getRandomValues.bind(crypto),
} as Crypto;

describe('uuidv4', () => {
  it('uses native randomUUID when the context provides it', () => {
    const canned = '11111111-2222-4333-8444-555555555555';
    const secure = { randomUUID: () => canned } as unknown as Crypto;
    expect(uuidv4(secure)).toBe(canned);
  });

  it('emits well-formed v4 UUIDs without randomUUID', () => {
    for (let i = 0; i < 64; i += 1) {
      expect(uuidv4(insecureCrypto)).toMatch(V4_SHAPE);
    }
  });

  it('does not repeat across calls', () => {
    const seen = new Set(Array.from({ length: 256 }, () => uuidv4(insecureCrypto)));
    expect(seen.size).toBe(256);
  });
});
