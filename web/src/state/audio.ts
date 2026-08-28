/**
 * The audio backend's own state: started, running, which card, why not.
 *
 * Every other document is a list of what the backend can see, and an
 * empty list looks exactly like "nothing plugged in". This is the one read
 * that tells the two apart — the exclusive layer with its card unopened
 * enumerates nothing at all — so the patch bays read it alongside their
 * device lists.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { components } from '../api/generated/schema';

export type AudioStatusDto = components['schemas']['AudioStatusDto'];

export interface AudioState {
  status: AudioStatusDto | null;
  set: (status: AudioStatusDto) => void;
}

export const useAudioStatus = create<AudioState>((set) => ({
  status: null,
  set: (status) => set({ status }),
}));

/** Side-effect-free read. */
export async function loadAudioStatus(): Promise<void> {
  const { data } = await $api.GET('/api/v1/audio');
  if (data) useAudioStatus.getState().set(data);
}
