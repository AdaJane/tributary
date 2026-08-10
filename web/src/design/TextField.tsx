import { useState } from 'react';

import styles from './TextField.module.css';

export interface TextFieldProps {
  /** aria-label — the field renders no visible label of its own. */
  label: string;
  value: string;
  /** Enter/blur commits the trimmed draft iff it differs from value; Esc reverts. */
  onCommit: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
}

/** An inset machine-text well. A null draft means "not editing", so
 * external value changes flow straight through until typing starts. */
export function TextField({
  label,
  value,
  onCommit,
  placeholder,
  disabled = false,
}: TextFieldProps) {
  const [draft, setDraft] = useState<string | null>(null);

  const commit = () => {
    if (draft !== null) {
      const trimmed = draft.trim();
      if (trimmed !== value) onCommit(trimmed);
    }
    setDraft(null);
  };

  return (
    <input
      type="text"
      className={styles.field}
      aria-label={label}
      value={draft ?? value}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') commit();
        if (e.key === 'Escape') setDraft(null);
      }}
    />
  );
}
