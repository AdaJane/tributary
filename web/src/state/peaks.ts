/**
 * The take document on screen: waveform peaks + track metadata for one
 * take at a time. Finished takes load wholesale; while recording, bins
 * stream in and the lanes grow. Live buffers are mutated in place and
 * published as views on a ~100 ms throttle so canvases don't repaint per
 * WS message.
 */
import { create } from 'zustand';

import type { TakePeaks } from '../features/tracks/peaks-parse';

export interface TrackMeta {
  file: string;
  channels: number;
  stripId: number | null;
  /** Samples were dropped and padded with silence in this track. */
  damaged: boolean;
  /** A MIDI sidecar was recorded alongside this lane's instrument. */
  midi: boolean;
}

/** Redraw throttle for live-growing lanes. */
const LIVE_BUMP_MS = 100;

interface LiveBuffer {
  buf: Int16Array;
  /** Live entries (2 per bin). */
  len: number;
}

let liveBuffers: LiveBuffer[] = [];
let bumpTimer: ReturnType<typeof setTimeout> | null = null;

/** A pending live bump must never outlive the document it described — a
 * stale timer firing after a swap would wipe the fresh tracks. */
function dropPendingBump(): void {
  if (bumpTimer !== null) {
    clearTimeout(bumpTimer);
    bumpTimer = null;
  }
}

function writeAt(buffer: LiveBuffer, startBin: number, bins: number[]): void {
  const needed = startBin * 2 + bins.length;
  if (needed > buffer.buf.length) {
    const grown = new Int16Array(Math.max(needed, buffer.buf.length * 2, 4096));
    grown.set(buffer.buf.subarray(0, buffer.len));
    buffer.buf = grown;
  }
  buffer.buf.set(bins, startBin * 2);
  buffer.len = Math.max(buffer.len, needed);
}

export interface PeaksState {
  /** Which take the document describes; null = nothing loaded. */
  take: number | null;
  samplesPerBin: number;
  /** Per track: interleaved (min, max) pairs. Index-aligned with meta. */
  tracks: Int16Array[];
  trackMeta: TrackMeta[];
  /** Recorded frames so far while live (max lane length × bin size). */
  liveFrames: number;
  status: 'idle' | 'loading' | 'ready' | 'error';
  /** Why the waveform is missing. Null while it is fine. */
  error: string | null;
  /** Start loading a take: empties the document first, so a failed load
   *  can never leave the PREVIOUS take's waveform under the new header. */
  beginLoad: (take: number) => void;
  failLoad: (message: string) => void;
  setPeaks: (take: number, peaks: TakePeaks, meta: TrackMeta[]) => void;
  /** Begin a live document: one empty growable lane per meta entry. */
  startLive: (take: number, meta: TrackMeta[]) => void;
  appendBins: (take: number, track: number, startBin: number, bins: number[]) => void;
  clear: () => void;
}

export const usePeaksStore = create<PeaksState>((set, get) => ({
  take: null,
  samplesPerBin: 512,
  tracks: [],
  trackMeta: [],
  liveFrames: 0,
  status: 'idle',
  error: null,
  beginLoad: (take) => {
    dropPendingBump();
    liveBuffers = [];
    set({
      take,
      tracks: [],
      trackMeta: [],
      liveFrames: 0,
      status: 'loading',
      error: null,
    });
  },
  failLoad: (error) => set({ status: 'error', error, tracks: [], trackMeta: [] }),
  setPeaks: (take, peaks, meta) => {
    dropPendingBump();
    liveBuffers = [];
    set({
      take,
      samplesPerBin: peaks.samplesPerBin,
      tracks: peaks.tracks,
      trackMeta: meta,
      liveFrames: 0,
      status: 'ready',
      error: null,
    });
  },
  startLive: (take, meta) => {
    dropPendingBump();
    liveBuffers = meta.map(() => ({ buf: new Int16Array(4096), len: 0 }));
    set({
      take,
      tracks: liveBuffers.map(() => new Int16Array(0)),
      trackMeta: meta,
      liveFrames: 0,
      status: 'ready',
      error: null,
    });
  },
  appendBins: (take, track, startBin, bins) => {
    if (take !== get().take) return;
    const buffer = liveBuffers[track];
    if (!buffer) return;
    writeAt(buffer, startBin, bins);
    if (bumpTimer !== null) return;
    bumpTimer = setTimeout(() => {
      bumpTimer = null;
      const spb = get().samplesPerBin;
      const maxLen = liveBuffers.reduce((n, b) => Math.max(n, b.len), 0);
      set({
        tracks: liveBuffers.map((b) => b.buf.subarray(0, b.len)),
        liveFrames: (maxLen / 2) * spb,
      });
    }, LIVE_BUMP_MS);
  },
  clear: () => {
    dropPendingBump();
    liveBuffers = [];
    set({
      take: null,
      tracks: [],
      trackMeta: [],
      liveFrames: 0,
      status: 'idle',
      error: null,
    });
  },
}));

// Dev console access to the live store (vite dynamic import would create a
// second, empty instance).
if (import.meta.env.DEV) {
  (window as unknown as Record<string, unknown>).__tribPeaks = usePeaksStore;
}
