import { useId } from 'react';

import styles from './CollapsibleSection.module.css';

export interface CollapsibleSectionProps {
  title: string;
  expanded: boolean;
  onToggle: (expanded: boolean) => void;
  /** One-line mono reading shown while collapsed ("HF +2 · 1.2k +4 · LF 0"). */
  summary?: string;
  /** Lights the section's state dot (e.g. EQ engaged). */
  engaged?: boolean;
  children: React.ReactNode;
}

/** A strip section that folds to save console real estate. */
export function CollapsibleSection({
  title,
  expanded,
  onToggle,
  summary,
  engaged = false,
  children,
}: CollapsibleSectionProps) {
  const contentId = useId();
  return (
    <section className={styles.section}>
      <button
        type="button"
        className={styles.header}
        aria-expanded={expanded}
        aria-controls={contentId}
        onClick={() => onToggle(!expanded)}
      >
        <span className={styles.dot} data-on={engaged || undefined} aria-hidden />
        <span className={styles.title}>{title}</span>
        <span className={styles.chevron} data-expanded={expanded || undefined} aria-hidden>
          ▾
        </span>
      </button>
      {!expanded && summary && <div className={styles.summary}>{summary}</div>}
      <div className={styles.fold} data-expanded={expanded || undefined}>
        {/* inert (not hidden): the fold animation needs the content in
            layout, but collapsed controls must not be reachable. */}
        <div
          id={contentId}
          className={styles.content}
          inert={!expanded}
          aria-hidden={!expanded}
        >
          {children}
        </div>
      </div>
    </section>
  );
}
