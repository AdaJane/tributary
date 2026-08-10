/**
 * The server-mirror of the mixer document. The daemon is authoritative at
 * rest; `setLocal` applies optimistic gesture values, and `state/echo.ts`
 * decides which incoming echoes to admit.
 */
import { create } from 'zustand';

import type { MixerState, StateDelta } from '../ws/messages';
import { mergeDelta } from './mixer-merge';

export const EMPTY_MIXER: MixerState = {
  strips: [],
  buses: [],
  fx: [],
  master: { fader_db: 0, record_arm: false },
};

interface MixerStore {
  state: MixerState;
  /** True once any snapshot has arrived — gates the console render. */
  loaded: boolean;
  applySnapshot: (state: MixerState) => void;
  applyDelta: (delta: StateDelta) => void;
  /** Optimistic local application of a gesture (same shape as an echo). */
  setLocal: (delta: StateDelta) => void;
}

export const useMixer = create<MixerStore>((set) => ({
  state: EMPTY_MIXER,
  loaded: false,
  applySnapshot: (state) => set({ state, loaded: true }),
  applyDelta: (delta) => set((s) => ({ state: mergeDelta(s.state, delta) })),
  setLocal: (delta) => set((s) => ({ state: mergeDelta(s.state, delta) })),
}));
