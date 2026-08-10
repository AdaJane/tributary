import { parseMonitorFrame } from './frame-parse';

/** Reconnect cadence while the stream is engaged but the socket is down. */
const RECONNECT_MS = 1_000;

/**
 * The browser end of the monitor: WS frames → worklet ring → speakers.
 * Owns one AudioContext + one socket; `stop()` releases both.
 */
export class MonitorPlayer {
  private context: AudioContext | null = null;
  private node: AudioWorkletNode | null = null;
  private socket: WebSocket | null = null;
  private running = false;

  async start(wsUrl: string): Promise<void> {
    this.running = true;
    const context = new AudioContext({ sampleRate: 48_000 });
    this.context = context;
    await context.audioWorklet.addModule(new URL('./monitor-processor.ts', import.meta.url));
    if (!this.running) return; // stopped while loading
    const node = new AudioWorkletNode(context, 'trib-monitor', {
      outputChannelCount: [2],
    });
    node.connect(context.destination);
    this.node = node;
    this.connect(wsUrl);
    // Autoplay policy: resume works silently when the page has been
    // interacted with; otherwise the caller shows "tap to enable".
    await context.resume().catch(() => undefined);
  }

  private connect(wsUrl: string): void {
    if (!this.running) return;
    const socket = new WebSocket(wsUrl);
    socket.binaryType = 'arraybuffer';
    socket.onmessage = (event) => {
      if (!(event.data instanceof ArrayBuffer)) return;
      const frame = parseMonitorFrame(event.data);
      if (frame && this.node) {
        this.node.port.postMessage(
          { generation: frame.generation, samples: frame.samples },
          [frame.samples.buffer],
        );
      }
    };
    socket.onclose = () => {
      if (this.running) setTimeout(() => this.connect(wsUrl), RECONNECT_MS);
    };
    this.socket = socket;
  }

  get suspended(): boolean {
    return this.context?.state === 'suspended';
  }

  async resume(): Promise<void> {
    await this.context?.resume();
  }

  stop(): void {
    this.running = false;
    this.socket?.close();
    this.socket = null;
    this.node?.disconnect();
    this.node = null;
    void this.context?.close().catch(() => undefined);
    this.context = null;
  }
}
