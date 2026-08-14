/**
 * The rear-panel card: a screen-printed header over a slab of panel metal.
 *
 * Lifted out of SetupView when the Instruments room needed the same
 * recipe. A panel card is a design fact — surface, depth, header type — and
 * a second copy of it in another stylesheet is a drift waiting to happen.
 */
import type { ReactNode } from 'react';

import styles from './Panel.module.css';

export function Panel({
  title,
  badge,
  children,
}: {
  title: string;
  /** Right-aligned header slot: a lamp, a count, a restart warning. */
  badge?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className={styles.panel}>
      <header className={styles.panelHeader}>
        <h2 className={styles.panelTitle}>{title}</h2>
        {badge}
      </header>
      {children}
    </section>
  );
}

/**
 * A refused save, said where it happened. `role="status"` so a screen
 * reader hears it without the focus moving; colour is never the only
 * signal, so the dot is joined by the sentence itself.
 */
export function InlineError({ message }: { message: string }) {
  return (
    <p className={styles.error} role="status">
      <span className={styles.errorDot} />
      {message}
    </p>
  );
}

/**
 * The panel body before the daemon has answered. Deliberately not a
 * spinner: nothing is spinning, the socket is simply not back yet.
 */
export function Waiting() {
  return <p className={styles.waiting}>waiting for the daemon…</p>;
}
