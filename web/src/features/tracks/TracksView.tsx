import { TapeLabel } from '../../design/TapeLabel';
import { useTransport } from '../../state/transport';
import { Timeline } from './Timeline';
import { TransportBar } from './TransportBar';
import { usePeaks } from './usePeaks';
import styles from './TracksView.module.css';

/** The session's tape: transport, then one waveform lane per track of the
 * latest take. */
export function TracksView() {
  usePeaks();
  const phase = useTransport((s) => s.phase);
  const take = useTransport((s) => s.take);

  const empty = take === null && phase !== 'recording';

  return (
    <div className={styles.tracks}>
      <header className={styles.header}>
        <div className={styles.title}>
          <TapeLabel id="tracks-title" name="Tracks" />
        </div>
        <TransportBar />
        {phase === 'recording' && <span className={styles.rolling}>tape rolling…</span>}
      </header>
      {empty ? (
        <p className={styles.empty}>No takes yet — arm a channel below and hit REC.</p>
      ) : (
        <Timeline />
      )}
    </div>
  );
}
