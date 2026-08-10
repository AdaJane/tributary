/**
 * Purely visual workspace state. Persisted: a console setup is a workspace,
 * and the daemon must never know about a screen-only concern.
 */
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

const DEFAULT_VIEW = 'console';

interface UiState {
  view: string;
  /** Per-strip EQ fold state, keyed by strip id. Default collapsed. */
  eqExpanded: Record<string, boolean>;
  setView: (view: string) => void;
  setEqExpanded: (stripId: string, expanded: boolean) => void;
}

export const useUi = create<UiState>()(
  persist(
    (set) => ({
      view: DEFAULT_VIEW,
      eqExpanded: {},
      setView: (view) => set({ view }),
      setEqExpanded: (stripId, expanded) =>
        set((s) => ({ eqExpanded: { ...s.eqExpanded, [stripId]: expanded } })),
    }),
    { name: 'tributary-ui' },
  ),
);
