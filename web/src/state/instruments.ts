/**
 * The instruments document: the rack, why each unit is or is not sounding,
 * what MIDI is available, and what there is to load.
 *
 * One store rather than three because the three answers are entangled —
 * deleting a soundfont changes an instrument's status, unplugging a
 * keyboard changes its reason — and separate stores would let the console
 * render a pair that never existed together.
 *
 * Non-optimistic throughout: every write here creates or destroys something
 * the daemon names (an id, a preset table, a strip), so predicting the
 * result would be guessing. Each action resolves to `null` on success or a
 * sentence to print.
 */
import { create } from 'zustand';

import { $api } from '../api/client';
import type { InstrumentReport } from '../features/console/instrument-logic';
import type { Preset } from '../features/instruments/preset-options';
import { instrumentErrorMessage } from '../features/console/instrument-logic';
import type { InstrumentSplit, InstrumentState } from '../ws/messages';

export interface MidiPortReport {
  id: string;
  name: string;
  connected: boolean;
  absent: boolean;
}

export interface SoundfontInfo {
  id: string;
  path: string;
  bytes: number;
  origin: 'built_in' | 'internal' | 'removable';
  volume?: string | null;
}

export interface InstrumentsDocument {
  instruments: InstrumentState[];
  reports: InstrumentReport[];
  midiPorts: MidiPortReport[];
  soundfonts: SoundfontInfo[];
  soundfontDir: string;
  maxUploadBytes: number;
}

interface InstrumentsStore {
  doc: InstrumentsDocument | null;
  loaded: boolean;
  /** Scoped by instrument so one unit's refusal never prints inside another. */
  error: { id: number | null; message: string } | null;
  pending: boolean;
  load: () => Promise<void>;
  refresh: () => Promise<void>;
  add: () => Promise<string | null>;
  remove: (id: number) => Promise<string | null>;
  update: (id: number, patch: Record<string, unknown>) => Promise<string | null>;
  setOutputs: (id: number, layout: OutputLayout) => Promise<string | null>;
  addStrips: (id: number) => Promise<string | null>;
  test: (id: number) => Promise<string | null>;
  panic: () => Promise<void>;
  deleteSoundfont: (name: string) => Promise<string | null>;
  clearError: () => void;
}

export type OutputLayout =
  | { kind: 'stereo_mix' }
  | { kind: 'gm_drums' }
  | { kind: 'custom'; splits: InstrumentSplit[] };

function toDoc(body: {
  instruments: InstrumentState[];
  reports: InstrumentReport[];
  midi_ports: MidiPortReport[];
  soundfonts: SoundfontInfo[];
  soundfont_dir: string;
  max_upload_bytes: number;
}): InstrumentsDocument {
  return {
    instruments: body.instruments,
    reports: body.reports,
    midiPorts: body.midi_ports,
    soundfonts: body.soundfonts,
    soundfontDir: body.soundfont_dir,
    maxUploadBytes: body.max_upload_bytes,
  };
}

export const useInstruments = create<InstrumentsStore>()((set, get) => ({
  doc: null,
  loaded: false,
  error: null,
  pending: false,

  clearError: () => set({ error: null }),

  load: async () => {
    const { data } = await $api.GET('/api/v1/instruments', {});
    if (data) set({ doc: toDoc(data), loaded: true });
  },

  refresh: async () => {
    set({ pending: true });
    const { data } = await $api.POST('/api/v1/instruments/refresh', {});
    set({ pending: false, ...(data ? { doc: toDoc(data), loaded: true } : {}) });
  },

  add: async () => {
    // Created with a voice already loaded where there is one to load, so
    // a fresh appliance makes a sound on the first press rather than
    // presenting an instrument that cannot.
    const { defaultSoundfont } = await import('../features/instruments/soundfont-logic');
    const soundfont = defaultSoundfont(get().doc?.soundfonts ?? []);
    const { error, response } = await $api.POST('/api/v1/instruments', {
      body: soundfont ? { soundfont } : {},
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id: null, message } });
      return message;
    }
    await get().load();
    return null;
  },

  remove: async (id) => {
    const { error, response } = await $api.DELETE('/api/v1/instruments/{id}', {
      params: { path: { id } },
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id, message } });
      return message;
    }
    await get().load();
    return null;
  },

  update: async (id, patch) => {
    const { error, response } = await $api.PUT('/api/v1/instruments/{id}', {
      params: { path: { id } },
      body: patch as never,
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id, message } });
      return message;
    }
    set({ error: null });
    await get().load();
    return null;
  },

  setOutputs: async (id, layout) => {
    const { error, response } = await $api.PUT('/api/v1/instruments/{id}/outputs', {
      params: { path: { id } },
      body: layout as never,
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id, message } });
      return message;
    }
    set({ error: null });
    await get().load();
    return null;
  },

  addStrips: async (id) => {
    const { error, response } = await $api.POST('/api/v1/instruments/{id}/strips', {
      params: { path: { id } },
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id, message } });
      return message;
    }
    set({ error: null });
    return null;
  },

  test: async (id) => {
    const { response } = await $api.POST('/api/v1/instruments/{id}/test', {
      params: { path: { id } },
    });
    if (!response.ok) {
      const message = instrumentErrorMessage(response.status);
      set({ error: { id, message } });
      return message;
    }
    return null;
  },

  panic: async () => {
    await $api.POST('/api/v1/instruments/panic', {});
  },

  deleteSoundfont: async (name) => {
    const { error, response } = await $api.DELETE('/api/v1/soundfonts/{name}', {
      params: { path: { name } },
    });
    if (error || !response.ok) {
      const message = instrumentErrorMessage(response.status, detailOf(error));
      set({ error: { id: null, message } });
      return message;
    }
    await get().load();
    return null;
  },
}));

function detailOf(error: unknown): string | undefined {
  if (error && typeof error === 'object' && 'detail' in error) {
    const detail = (error as { detail?: unknown }).detail;
    if (typeof detail === 'string') return detail;
  }
  return undefined;
}

/**
 * The presets inside one library file.
 *
 * Its own request, cached by the caller: a General MIDI bank holds ~300 and
 * reading them means parsing a file that can be 206 MB, so this is asked
 * once per soundfont rather than folded into the instruments document that
 * every mixer change refetches.
 */
export async function fetchPresets(name: string): Promise<Preset[]> {
  const { data, error } = await $api.GET('/api/v1/soundfonts/{name}/presets', {
    params: { path: { name } },
  });
  if (error || !data) return [];
  return data as Preset[];
}
