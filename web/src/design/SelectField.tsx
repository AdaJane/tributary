import type { SelectOption } from './select-options';
import { withCurrentValue } from './select-options';
import styles from './SelectField.module.css';

export interface SelectFieldProps {
  label: string;
  value: string;
  options: readonly SelectOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
  /** Right-hand annotation: a count, a state word, a unit. */
  hint?: string;
}

/**
 * The unbounded picker: a native `<select>` in an inset well, for lists the
 * machine supplies and nobody can be expected to scan (presets, ports,
 * output channels).
 *
 * `SegmentedControl` stops working past about five options and this is what
 * replaces it — the two are not interchangeable, because a fixed short set
 * of choices is still piano keys.
 */
export function SelectField({
  label,
  value,
  options,
  onChange,
  disabled,
  hint,
}: SelectFieldProps) {
  const id = `select-${label.replace(/\s+/g, '-').toLowerCase()}`;
  return (
    <p className={styles.field}>
      <label className={styles.label} htmlFor={id}>
        {label}
      </label>
      <select
        id={id}
        className={styles.select}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      >
        {withCurrentValue(options, value).map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {hint && <span className={styles.hint}>{hint}</span>}
    </p>
  );
}
