import { useRef } from 'react';

import styles from './SegmentedControl.module.css';

export interface SegmentedOption<V extends string> {
  value: V;
  label: string;
  ariaLabel?: string;
}

export interface SegmentedControlProps<V extends string> {
  /** Group name for assistive tech, e.g. "File format". */
  label: string;
  options: readonly SegmentedOption<V>[];
  value: V;
  onChange: (value: V) => void;
  disabled?: boolean;
}

/** A row of interlocked latching switches — exactly one cap stays down.
 * Radio-group keyboarding: arrows move (and select, wrapping), Home/End
 * jump, only the checked cap is tabbable. */
export function SegmentedControl<V extends string>({
  label,
  options,
  value,
  onChange,
  disabled = false,
}: SegmentedControlProps<V>) {
  const group = useRef<HTMLDivElement>(null);

  const checkedIndex = options.findIndex((option) => option.value === value);
  const tabbableIndex = checkedIndex === -1 ? 0 : checkedIndex;

  const onKeyDown = (e: React.KeyboardEvent) => {
    const select = (index: number) => {
      e.preventDefault();
      const wrapped = (index + options.length) % options.length;
      onChange(options[wrapped].value);
      group.current?.querySelectorAll('button')[wrapped]?.focus();
    };
    switch (e.key) {
      case 'ArrowRight':
      case 'ArrowDown':
        return select(tabbableIndex + 1);
      case 'ArrowLeft':
      case 'ArrowUp':
        return select(tabbableIndex - 1);
      case 'Home':
        return select(0);
      case 'End':
        return select(options.length - 1);
    }
  };

  return (
    <div ref={group} className={styles.group} role="radiogroup" aria-label={label}>
      {options.map((option, index) => {
        const checked = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            className={styles.cap}
            aria-checked={checked}
            aria-label={option.ariaLabel}
            tabIndex={index === tabbableIndex ? 0 : -1}
            disabled={disabled}
            data-checked={checked || undefined}
            onClick={checked ? undefined : () => onChange(option.value)}
            onKeyDown={onKeyDown}
          >
            <span className={styles.led} aria-hidden />
            <span className={styles.label}>{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
