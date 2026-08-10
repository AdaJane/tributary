import { useConnection } from '../state/connection';
import styles from './ConnectionBadge.module.css';

const STATUS_LABEL = {
  connecting: 'Linking',
  online: 'Online',
  offline: 'Offline',
} as const;

/** Link-state LED, like the power lamp on the console's meter bridge. */
export function ConnectionBadge() {
  const status = useConnection((s) => s.status);
  return (
    <div className={styles.badge}>
      <span className={styles.led} data-status={status} aria-hidden />
      <span className={styles.label}>{STATUS_LABEL[status]}</span>
    </div>
  );
}
