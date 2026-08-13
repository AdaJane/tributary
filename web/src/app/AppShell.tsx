import { Suspense, lazy } from 'react';

import { useSessionFeed } from '../state/useSessionFeed';
import { useUi } from '../state/ui';
import styles from './AppShell.module.css';
import { ConnectionBadge } from './ConnectionBadge';
import { ViewPager } from './ViewPager';

const Bench = lazy(() => import('../design/Bench').then((m) => ({ default: m.Bench })));
const Setup = lazy(() =>
  import('../features/setup/SetupView').then((m) => ({ default: m.SetupView })),
);

const TABS = [
  { id: 'console', title: 'Console' },
  { id: 'tracks', title: 'Tracks' },
  { id: 'setup', title: 'Setup' },
  ...(import.meta.env.DEV ? [{ id: 'bench', title: 'Bench' }] : []),
];

export function AppShell() {
  const view = useUi((s) => s.view);
  const setView = useUi((s) => s.setView);
  const active = TABS.some((t) => t.id === view) ? view : 'console';

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
        {active === 'bench' ? (
          <Suspense fallback={null}>
            <Bench />
          </Suspense>
        ) : active === 'setup' ? (
          <Suspense fallback={null}>
            <Setup />
          </Suspense>
        ) : (
          <ViewPager />
        )}
      </main>
    </div>
  );
}
