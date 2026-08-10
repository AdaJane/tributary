import styles from './PushButton.module.css';

export type ButtonVariant = 'mute' | 'pfl' | 'arm' | 'solo' | 'plain';

export interface PushButtonProps {
  label: string;
  /** Full description for assistive tech, e.g. "Mute channel 3". */
  ariaLabel?: string;
  variant: ButtonVariant;
  pressed: boolean;
  onToggle: (pressed: boolean) => void;
  /** ARM while the transport records. Reduced motion renders steady. */
  blinking?: boolean;
  /** Hide the integrated LED when the status lamp lives elsewhere (e.g.
   * the EQ dot in the section header). The cap still depresses. */
  led?: boolean;
  disabled?: boolean;
}

/** A latching hardware button with an integrated LED. */
export function PushButton({
  label,
  ariaLabel,
  variant,
  pressed,
  onToggle,
  blinking = false,
  led = true,
  disabled = false,
}: PushButtonProps) {
  return (
    <button
      type="button"
      className={styles.button}
      aria-label={ariaLabel ?? label}
      aria-pressed={pressed}
      disabled={disabled}
      data-variant={variant}
      data-pressed={pressed || undefined}
      data-blinking={(blinking && pressed) || undefined}
      onClick={() => onToggle(!pressed)}
    >
      {led && <span className={styles.led} aria-hidden />}
      <span className={styles.label}>{label}</span>
    </button>
  );
}
