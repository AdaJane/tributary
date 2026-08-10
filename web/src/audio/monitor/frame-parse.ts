/**
 * Parser for `/ws/monitor` binary frames (little-endian, 24-byte header):
 * `"TMON"` · generation u32 · start_frame u64 · frame_count u32 · flags
 * u32 · frame_count × 2 × f32 interleaved stereo.
 */

export interface MonitorFrame {
  generation: number;
  startFrame: number;
  frameCount: number;
  /** Interleaved stereo, viewing the message buffer (offset 24 is 4-byte
   * aligned by design). */
  samples: Float32Array;
}

export function parseMonitorFrame(buffer: ArrayBuffer): MonitorFrame | null {
  if (buffer.byteLength < 24) return null;
  const view = new DataView(buffer);
  const magicOk =
    view.getUint8(0) === 0x54 && // T
    view.getUint8(1) === 0x4d && // M
    view.getUint8(2) === 0x4f && // O
    view.getUint8(3) === 0x4e; // N
  if (!magicOk) return null;
  const generation = view.getUint32(4, true);
  const startFrame = Number(view.getBigUint64(8, true));
  const frameCount = view.getUint32(16, true);
  if (buffer.byteLength < 24 + frameCount * 8) return null;
  return {
    generation,
    startFrame,
    frameCount,
    samples: new Float32Array(buffer, 24, frameCount * 2),
  };
}
