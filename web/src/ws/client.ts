/**
 * The single WebSocket to the daemon: auth-then-resubscribe on every
 * (re)connect, refcounted subscriptions, jittered-backoff reconnect, and a
 * ping keepalive that detects a silently dead socket.
 */
import { useConnection } from '../state/connection';
import { backoffDelay } from './backoff';
import {
  type Channel,
  type ClientMessage,
  type ServerMessage,
  channelKey,
  parseServerMessage,
} from './messages';

const PING_INTERVAL_MS = 15_000;
/** No traffic for this long = the socket is dead even if open. */
const STALE_MS = 30_000;

export type MessageHandler = (message: ServerMessage) => void;

export class WsClient {
  private socket: WebSocket | null = null;
  private handlers = new Set<MessageHandler>();
  private subscriptions = new Map<string, { channel: Channel; count: number }>();
  private attempt = 0;
  private lastTraffic = 0;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private stopped = true;
  private readonly url: string;

  constructor(url: string) {
    this.url = url;
  }

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer);
    if (this.pingTimer !== null) clearInterval(this.pingTimer);
    this.pingTimer = null;
    this.socket?.close();
    this.socket = null;
  }

  onMessage(handler: MessageHandler): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  /** Refcounted: the first interested party subscribes on the wire, the
   * last one unsubscribes. Returns the release function. */
  subscribe(channel: Channel): () => void {
    const key = channelKey(channel);
    const entry = this.subscriptions.get(key);
    if (entry) {
      entry.count += 1;
    } else {
      this.subscriptions.set(key, { channel, count: 1 });
      this.send({ op: 'subscribe', channel });
    }
    let released = false;
    return () => {
      if (released) return;
      released = true;
      const current = this.subscriptions.get(key);
      if (!current) return;
      current.count -= 1;
      if (current.count === 0) {
        this.subscriptions.delete(key);
        this.send({ op: 'unsubscribe', channel });
      }
    };
  }

  /** Fire-and-forget while disconnected: state converges via the snapshot
   * on reconnect, so queuing stale gestures would only replay the past. */
  send(message: ClientMessage): void {
    if (this.socket?.readyState === WebSocket.OPEN) {
      this.socket.send(JSON.stringify(message));
    }
  }

  private connect(): void {
    if (this.stopped) return;
    useConnection.getState().setStatus('connecting');
    const socket = new WebSocket(this.url);
    this.socket = socket;

    socket.onopen = () => {
      this.attempt = 0;
      this.lastTraffic = Date.now();
      useConnection.getState().setStatus('online');
      for (const { channel } of this.subscriptions.values()) {
        this.send({ op: 'subscribe', channel });
      }
      this.pingTimer ??= setInterval(() => this.keepalive(), PING_INTERVAL_MS);
    };

    socket.onmessage = (event) => {
      this.lastTraffic = Date.now();
      const message = parseServerMessage(String(event.data));
      if (!message) return;
      for (const handler of this.handlers) {
        handler(message);
      }
    };

    socket.onclose = () => {
      if (this.socket !== socket) return; // superseded
      this.socket = null;
      useConnection.getState().setStatus('offline');
      this.scheduleReconnect();
    };

    socket.onerror = () => socket.close();
  }

  private keepalive(): void {
    if (!this.socket) return;
    if (Date.now() - this.lastTraffic > STALE_MS) {
      // Dead peer: close() triggers onclose → reconnect.
      this.socket.close();
      return;
    }
    this.send({ op: 'ping' });
  }

  private scheduleReconnect(): void {
    if (this.stopped || this.reconnectTimer !== null) return;
    const delay = backoffDelay(this.attempt, Math.random);
    this.attempt += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, delay);
  }
}
