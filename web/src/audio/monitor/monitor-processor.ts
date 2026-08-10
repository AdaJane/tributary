/**
 * The monitor AudioWorklet: an interleaved-stereo ring the main thread
 * fills from WS frames; `process` deinterleaves into the output quantum.
 * Underrun plays silence; overflow drops the oldest audio; a generation
 * change flushes so two sessions never splice.
 *
 * The ring class is exported for vitest; `registerProcessor` only exists
 * inside the AudioWorkletGlobalScope, so registration is guarded.
 */

/** ~500 ms of interleaved stereo at 48 kHz. */
const RING_CAPACITY = 48_000;

export class MonitorRing {
  private buf: Float32Array;
  private read = 0;
  private count = 0;

  constructor(capacity: number = RING_CAPACITY) {
    this.buf = new Float32Array(capacity);
  }

  available(): number {
    return this.count;
  }

  clear(): void {
    this.read = 0;
    this.count = 0;
  }

  /** Overflow drops the OLDEST samples — late audio is worse than a skip. */
  write(samples: Float32Array): void {
    const cap = this.buf.length;
    for (const sample of samples) {
      if (this.count === cap) {
        this.read = (this.read + 1) % cap;
        this.count -= 1;
      }
      this.buf[(this.read + this.count) % cap] = sample;
      this.count += 1;
    }
  }

  /** Deinterleave into planar L/R; the shortfall stays silent. */
  readStereo(left: Float32Array, right: Float32Array): void {
    const cap = this.buf.length;
    for (let i = 0; i < left.length; i += 1) {
      if (this.count >= 2) {
        left[i] = this.buf[this.read];
        right[i] = this.buf[(this.read + 1) % cap];
        this.read = (this.read + 2) % cap;
        this.count -= 2;
      } else {
        left[i] = 0;
        right[i] = 0;
      }
    }
  }
}

declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort;
  abstract process(
    inputs: Float32Array[][],
    outputs: Float32Array[][],
    parameters: Record<string, Float32Array>,
  ): boolean;
}
declare function registerProcessor(
  name: string,
  ctor: new () => AudioWorkletProcessor,
): void;

interface MonitorMessage {
  generation: number;
  samples: Float32Array;
}

if (typeof registerProcessor === 'function') {
  class MonitorProcessor extends AudioWorkletProcessor {
    private ring = new MonitorRing();
    private generation = -1;

    constructor() {
      super();
      this.port.onmessage = (event: MessageEvent<MonitorMessage>) => {
        const { generation, samples } = event.data;
        if (generation !== this.generation) {
          this.generation = generation;
          this.ring.clear();
        }
        this.ring.write(samples);
      };
    }

    process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
      const output = outputs[0];
      if (output?.length >= 2) {
        this.ring.readStereo(output[0], output[1]);
      }
      return true;
    }
  }
  registerProcessor('trib-monitor', MonitorProcessor);
}
