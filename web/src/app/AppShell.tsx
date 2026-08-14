import { Suspense, lazy } from 'react';
import type { ComponentType } from 'react';

import { useSessionFeed } from '../state/useSessionFeed';
import { useUi } from '../state/ui';
import styles from './AppShell.module.css';
import { ConnectionBadge } from './ConnectionBadge';
import { ViewPager } from './ViewPager';

const Bench = lazy(() => import('../design/Bench').then((m) => ({ default: m.Bench })));
const Setup = lazy(() =>
  import('../features/setup/SetupView').then((m) => ({ default: m.SetupView })),
);
const Instruments = lazy(() =>
  import('../features/instruments/InstrumentsView').then((m) => ({
    default: m.InstrumentsView,
  })),
);

const TABS = [
  { id: 'console', title: 'Console' },
  { id: 'tracks', title: 'Tracks' },
  // Between the live rooms and the rear panel: you change a preset far
  // more often than a sample rate.
  { id: 'instruments', title: 'Instruments' },
  { id: 'setup', title: 'Setup' },
  ...(import.meta.env.DEV ? [{ id: 'bench', title: 'Bench' }] : []),
];

/**
 * Views that replace the pager rather than living inside it. A table
 * rather than a ternary chain: a third full-page room is where nesting
 * conditionals stops being readable, and a fourth is inevitable.
 */
const FULL_PAGE: Record<string, ComponentType> = {
  bench: Bench,
  setup: Setup,
  instruments: Instruments,
};

export function AppShell() {
  const view = useUi((s) => s.view);
  const setView = useUi((s) => s.setView);
  const active = TABS.some((t) => t.id === view) ? view : 'console';
  const FullPage = FULL_PAGE[active];

  useSessionFeed();

  return (
    <div className={styles.shell}>
      <header className={styles.topBar}>
        <span className={styles.brand}>Tributary</span>
        <nav className={styles.tabs} aria-label="Views">
          {TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              className={styles.tab}
              aria-current={tab.id === active ? 'page' : undefined}
              onClick={() => setView(tab.id)}
            >
              {tab.title}
            </button>
          ))}
        </nav>
        <div className={styles.status}>
          <ConnectionBadge />
        </div>
      </header>
      <main className={styles.main}>
        {FullPage ? (
          <Suspense fallback={null}>
            <FullPage />
          </Suspense>
        ) : (
          <ViewPager />
        )}
      </main>
    </div>
  );
}
