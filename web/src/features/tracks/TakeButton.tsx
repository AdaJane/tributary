import { useTransport } from '../../state/transport';
import { takeLabel } from './take-list';
import styles from './TakeButton.module.css';

/**
 * Which take is on the machine, and the way to a different one.
 *
 * Prints the selection even when the browser is closed — the same job
 * `InputButton` does for the patchbay. The OLD chip is what tells you at a
 * glance that you are reviewing history rather than the last thing you
 * cut, which is otherwise invisible.
 */
export function TakeButton({ onPress }: { onPress: () => void }) {
  const take = useTransport((s) => s.take);
  const latest = useTransport((s) => s.latestTake);
  const reviewing = take !== null && latest !== null && take !== latest;

  return (
    <button
      type="button"
      className={styles.button}
      aria-haspopup="dialog"
      aria-label={take === null ? 'Browse takes' : `Take ${take} — browse takes`}
      onClick={onPress}
    >
      <span className={styles.label}>Take</span>
      <span className={styles.value}>{take === null ? '—' : takeLabel(take)}</span>
      {reviewing && <span className={styles.old}>OLD</span>}
    </button>
  );
}
