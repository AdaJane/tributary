import { useEffect, useRef, useState } from 'react';
import useFitText from 'use-fit-text';

import styles from './TapeLabel.module.css';
import { rotationFor } from './tape';

export interface TapeLabelProps {
  /** Stable identity — drives the deterministic tape lean. */
  id: string;
  name: string;
  /** Absent = read-only tape (no rename affordance). */
  onRename?: (name: string) => void;
  onSelect?: () => void;
}

/**
 * Off-screen twin of the tape's writing area: use-fit-text binary-searches
 * a font size that lets `text` wrap fully inside it, and reports the
 * result up. Keyed by the text so every change re-measures from scratch —
 * remounting the probe is free, remounting the input would eat the caret.
 */
function FitProbe({ text, onFit }: { text: string; onFit: (size: string) => void }) {
  const { fontSize, ref } = useFitText({ maxFontSize: 100, minFontSize: 50, resolution: 5 });
  useEffect(() => onFit(fontSize), [fontSize, onFit]);
  return (
    <div ref={ref} className={styles.probe} aria-hidden>
      {text}
    </div>
  );
}

/** Masking tape with a sharpie name. Long names wrap and the writing
 * shrinks to fit the tape. Double-click (or Enter/F2) renames inline;
 * Enter/blur commits, Esc reverts. */
export function TapeLabel({ id, name, onRename, onSelect }: TapeLabelProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const [fontSize, setFontSize] = useState('100%');
  const buttonRef = useRef<HTMLButtonElement>(null);
  const refocus = useRef(false);
  const editable = onRename !== undefined;

  // The tape button doesn't exist until the input unmounts, so focus must
  // wait for the post-close render.
  useEffect(() => {
    if (draft === null && refocus.current) {
      refocus.current = false;
      buttonRef.current?.focus();
    }
  }, [draft]);

  const close = () => {
    refocus.current = true;
    setDraft(null);
  };

  const commit = () => {
    if (draft !== null) {
      const trimmed = draft.trim();
      if (trimmed && trimmed !== name) onRename?.(trimmed);
    }
    close();
  };

  const text = draft ?? name;
  const style = { rotate: `${rotationFor(id)}deg` };

  return (
    <div className={styles.wrap} style={style}>
      <FitProbe key={text} text={text} onFit={setFontSize} />
      {draft !== null ? (
        <textarea
          className={styles.tape}
          style={{ fontSize }}
          aria-label="Channel name"
          value={draft}
          autoFocus
          rows={2}
          onChange={(e) => setDraft(e.target.value.replace(/\n/g, ' '))}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault(); // tape takes no newlines
              commit();
            }
            if (e.key === 'Escape') close();
          }}
        />
      ) : (
        <button
          ref={buttonRef}
          type="button"
          className={styles.tape}
          style={{ fontSize }}
          aria-label={
            editable ? `Rename channel, currently “${name}”` : `Channel ${name}`
          }
          onClick={onSelect}
          onDoubleClick={editable ? () => setDraft(name) : undefined}
          onKeyDown={
            editable
              ? (e) => {
                  if (e.key === 'Enter' || e.key === 'F2') setDraft(name);
                }
              : undefined
          }
        >
          {name}
        </button>
      )}
    </div>
  );
}
