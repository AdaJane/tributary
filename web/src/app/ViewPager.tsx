import { ChevronDown, ChevronUp } from 'lucide-react';
import { Suspense, lazy, useRef } from 'react';

import { useUi } from '../state/ui';
import styles from './ViewPager.module.css';
import { type Point, swipeDirection } from './swipe';

const ConsoleView = lazy(() =>
  import('../features/console/ConsoleView').then((m) => ({ default: m.ConsoleView })),
);
const TracksView = lazy(() =>
  import('../features/tracks/TracksView').then((m) => ({ default: m.TracksView })),
);

/**
 * The two rooms of the app, stacked spatially: Tracks lives ABOVE the
 * console. Chevron buttons ride each page's edge; on touch, pulling down
 * reveals Tracks (shade-style) and pushing up returns. Both pages stay
 * mounted so the console's feeds keep running while you read takes.
 */
export function ViewPager() {
  const view = useUi((s) => s.view);
  const setView = useUi((s) => s.setView);
  const onTracks = view === 'tracks';
  const touchStart = useRef<Point | null>(null);

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.pointerType === 'touch') {
      touchStart.current = { x: e.clientX, y: e.clientY };
    }
  };

  const onPointerUp = (e: React.PointerEvent) => {
    const start = touchStart.current;
    touchStart.current = null;
    if (!start || e.pointerType !== 'touch') return;
    const direction = swipeDirection(start, { x: e.clientX, y: e.clientY });
    if (direction === 'down' && !onTracks) setView('tracks');
    if (direction === 'up' && onTracks) setView('console');
  };

  return (
    <div
      className={styles.pager}
      onPointerDown={onPointerDown}
      onPointerUp={onPointerUp}
      onPointerCancel={() => (touchStart.current = null)}
    >
      <div className={styles.pages} data-view={onTracks ? 'tracks' : 'console'}>
        <section className={styles.page} inert={!onTracks} aria-label="Tracks">
          <Suspense fallback={null}>
            <TracksView />
          </Suspense>
          <button
            type="button"
            className={`${styles.jump} ${styles.jumpBottom}`}
            onClick={() => setView('console')}
          >
            View Console <ChevronDown size={13} aria-hidden />
          </button>
        </section>
        <section className={styles.page} inert={onTracks} aria-label="Console">
          <button
            type="button"
            className={`${styles.jump} ${styles.jumpTop}`}
            onClick={() => setView('tracks')}
          >
            View Tracks <ChevronUp size={13} aria-hidden />
          </button>
          <Suspense fallback={null}>
            <ConsoleView />
          </Suspense>
        </section>
      </div>
    </div>
  );
}
