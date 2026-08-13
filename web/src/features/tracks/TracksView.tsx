import { useState } from 'react';

import { TapeLabel } from '../../design/TapeLabel';
import { usePeaksStore } from '../../state/peaks';
import { useTransport } from '../../state/transport';
import { TakeBrowserModal } from './TakeBrowserModal';
import { TakeButton } from './TakeButton';
import { Timeline } from './Timeline';
import { TransportBar } from './TransportBar';
import { usePeaks } from './usePeaks';
import { useTakesFeed } from './useTakes';
import styles from './TracksView.module.css';

/** The session's tape: transport, then one waveform lane per track of the
 * selected take — any take of the open session, not only the newest. */
export function TracksView() {
  usePeaks();
  useTakesFeed();
  const phase = useTransport((s) => s.phase);
  const take = useTransport((s) => s.take);
  const peaksError = usePeaksStore((s) => s.error);
  const [browsing, setBrowsing] = useState(false);

  const empty = take === null && phase !== 'recording';

  return (
    <div className={styles.tracks}>
      <header className={styles.header}>
        <div className={styles.title}>
          <TapeLabel id="tracks-title" name="Tracks" />
        </div>
        <TakeButton onPress={() => setBrowsing(true)} />
        <TransportBar />
        {phase === 'recording' && <span className={styles.rolling}>tape rolling…</span>}
      </header>
      {/* A waveform that fails to load used to render as an empty room
          with no explanation. Picking a take makes that reachable on
          purpose, so it has to say something. */}
      {peaksError && (
        <p className={styles.loadError} role="alert">
          {peaksError}
        </p>
      )}
      {empty ? (
        <p className={styles.empty}>No takes yet — arm a channel below and hit REC.</p>
      ) : (
        <Timeline />
      )}
      {browsing && <TakeBrowserModal open onClose={() => setBrowsing(false)} />}
    </div>
  );
}
