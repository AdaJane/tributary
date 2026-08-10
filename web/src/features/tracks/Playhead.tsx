import { useEffect, useRef } from 'react';

import { usePeaksStore } from '../../state/peaks';
import { useTransport } from '../../state/transport';
import { estimatePosition } from './playhead-interp';
import styles from './Playhead.module.css';

export interface PlayheadProps {
  fpp: number;
  /** Pixel offset of the wave area (the lane-header column width). */
  headerW: number;
  getScrollLeft: () => number;
  /** Visible wave-area width. */
  viewWidth: number;
  /** Fired every frame with the playhead's content-space px — the follow hook. */
  onTick: (contentPx: number) => void;
}

/** The playhead line, moved by rAF outside React: reads the transport
 * store imperatively and interpolates between 20 Hz daemon updates. */
export function Playhead({ fpp, headerW, getScrollLeft, viewWidth, onTick }: PlayheadProps) {
  const lineRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let raf = 0;
    const tick = () => {
      // The console page keeps Tracks mounted (ViewPager); don't spend
      // frames positioning a playhead nobody can see.
      if (lineRef.current?.closest('[inert]')) {
        raf = requestAnimationFrame(tick);
        return;
      }
      const t = useTransport.getState();
      // Recording: the head is the last written bin — sample truth from
      // the writer, not a clock.
      const estimated =
        t.phase === 'recording'
          ? usePeaksStore.getState().liveFrames
          : estimatePosition(
              t.positionFrames,
              t.positionAtMs,
              performance.now(),
              t.sampleRate,
              t.phase === 'playing',
              t.totalFrames,
              t.loop,
            );
      const contentPx = estimated / fpp;
      onTick(contentPx);
      const x = contentPx - getScrollLeft();
      const el = lineRef.current;
      if (el) {
        const visible = x >= -1 && x <= viewWidth + 1;
        el.style.opacity = visible ? '1' : '0';
        el.style.transform = `translateX(${headerW + x}px)`;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [fpp, headerW, viewWidth, onTick, getScrollLeft]);

  return <div ref={lineRef} className={styles.playhead} aria-hidden />;
}
