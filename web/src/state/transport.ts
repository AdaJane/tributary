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
  /** Recording: the take being cut. Otherwise: the latest take. */
  take: number | null;
  startedAtUnix: number | null;
  positionFrames: number;
  /** performance.now() at the last position update — playhead interpolation. */
  positionAtMs: number;
  sampleRate: number;
  totalFrames: number;
  loop: LoopRegion | null;
  monitor: 'hardware' | 'stream';
  lanes: Lane[];
  apply: (dto: TransportDto) => void;
  applyPosition: (frames: number) => void;
}

export const useTransport = create<TransportState>((set) => ({
  phase: 'stopped',
  take: null,
  startedAtUnix: null,
  positionFrames: 0,
  positionAtMs: 0,
  sampleRate: 48_000,
  totalFrames: 0,
  loop: null,
  monitor: 'hardware',
  lanes: [],
  apply: (dto) =>
    set({
      phase: dto.state,
      take: dto.take ?? null,
      startedAtUnix: dto.started_at_unix ?? null,
      positionFrames: dto.position_frames,
      positionAtMs: performance.now(),
      sampleRate: dto.sample_rate,
      totalFrames: dto.total_frames,
      loop: dto.loop
        ? { startFrames: dto.loop.start_frames, endFrames: dto.loop.end_frames }
        : null,
      monitor: dto.monitor,
      lanes: dto.lanes.map((lane) => ({ solo: lane.solo, mute: lane.mute })),
    }),
  applyPosition: (frames) => set({ positionFrames: frames, positionAtMs: performance.now() }),
}));
