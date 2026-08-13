import { create } from 'zustand';

import type { TransportDto } from '../ws/messages';

export type TransportPhase = 'stopped' | 'playing' | 'recording';

export interface Lane {
  solo: boolean;
  mute: boolean;
}

export interface LoopRegion {
  startFrames: number;
  endFrames: number;
}

/**
 * Mirror of the daemon's transport. `positionFrames` updates at the pump
 * rate while playing — components must select primitives, never the whole
 * store, or they repaint at 20 Hz.
 */
export interface TransportState {
  phase: TransportPhase;
  /** The selected take — what PLAY rolls. Recording: the take being cut. */
  take: number | null;
  /** The newest take on the shelf; differs from `take` while reviewing. */
  latestTake: number | null;
  startedAtUnix: number | null;
  positionFrames: number;
  /** performance.now() at the last position update — playhead interpolation. */
  positionAtMs: number;
  /** The selected take's rate; the engine's own while recording. */
  sampleRate: number;
  /** What the engine runs at. A take cut at another rate can be viewed
   *  but not played, and the console needs both numbers to say so. */
  engineSampleRate: number;
  totalFrames: number;
  loop: LoopRegion | null;
  monitor: 'hardware' | 'stream';
  lanes: Lane[];
  apply: (dto: TransportDto) => void;
  applyPosition: (take: number, frames: number) => void;
}

export const useTransport = create<TransportState>((set) => ({
  phase: 'stopped',
  take: null,
  latestTake: null,
  startedAtUnix: null,
  positionFrames: 0,
  positionAtMs: 0,
  sampleRate: 48_000,
  engineSampleRate: 48_000,
  totalFrames: 0,
  loop: null,
  monitor: 'hardware',
  lanes: [],
  apply: (dto) =>
    set({
      phase: dto.state,
      take: dto.take ?? null,
      latestTake: dto.latest_take ?? null,
      startedAtUnix: dto.started_at_unix ?? null,
      positionFrames: dto.position_frames,
      positionAtMs: performance.now(),
      sampleRate: dto.sample_rate,
      engineSampleRate: dto.engine_sample_rate,
      totalFrames: dto.total_frames,
      loop: dto.loop
        ? { startFrames: dto.loop.start_frames, endFrames: dto.loop.end_frames }
        : null,
      monitor: dto.monitor,
      lanes: dto.lanes.map((lane) => ({ solo: lane.solo, mute: lane.mute })),
    }),
  // A tick queued before a take switch can land after it. Ignoring one
  // for a take we are no longer on stops it dragging the new take's
  // playhead — the message has always carried `take`; nothing read it.
  applyPosition: (take, frames) =>
    set((s) =>
      take === s.take ? { positionFrames: frames, positionAtMs: performance.now() } : {},
    ),
}));
