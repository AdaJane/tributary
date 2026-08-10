/**
 * Parser for the daemon's binary peaks document (see
 * `GET /api/v1/takes/{take}/peaks`): `"TPKS"` · version u8 · pad ·
 * track_count u16 · samples_per_bin u32 · per track: bin_count u32 +
 * (min i16, max i16) pairs. Little-endian throughout.
 */

export interface TakePeaks {
  samplesPerBin: number;
  /** Per track: interleaved (min, max) pairs, 2 entries per bin. */
  tracks: Int16Array[];
}

export function parsePeaks(buffer: ArrayBuffer): TakePeaks | null {
  const view = new DataView(buffer);
  if (buffer.byteLength < 12) return null;
  const magic = String.fromCharCode(
    view.getUint8(0),
    view.getUint8(1),
    view.getUint8(2),
    view.getUint8(3),
  );
  if (magic !== 'TPKS' || view.getUint8(4) !== 1) return null;
  const trackCount = view.getUint16(6, true);
  const samplesPerBin = view.getUint32(8, true);
  const tracks: Int16Array[] = [];
  let offset = 12;
  for (let t = 0; t < trackCount; t += 1) {
    if (offset + 4 > buffer.byteLength) return null;
    const binCount = view.getUint32(offset, true);
    offset += 4;
    const byteLength = binCount * 4;
    if (offset + byteLength > buffer.byteLength) return null;
    // The body is 4-byte aligned per track only when bin counts are even;
    // copy instead of aliasing so alignment never matters.
    const pairs = new Int16Array(binCount * 2);
    for (let i = 0; i < pairs.length; i += 1) {
      pairs[i] = view.getInt16(offset + i * 2, true);
    }
    offset += byteLength;
    tracks.push(pairs);
  }
  return { samplesPerBin, tracks };
}
