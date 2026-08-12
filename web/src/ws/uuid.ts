/**
 * RFC 4122 v4 UUID that also works outside secure contexts.
 * `crypto.randomUUID` exists only on HTTPS/localhost origins, and the
 * appliance serves plain HTTP on the LAN (http://tributary.local:4600);
 * `getRandomValues` is available everywhere.
 */
export function uuidv4(c: Crypto = crypto): string {
  if (typeof c.randomUUID === 'function') return c.randomUUID();
  const bytes = c.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
  bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join('-');
}
