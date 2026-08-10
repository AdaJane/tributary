import styles from './ActionButton.module.css';

export interface ActionButtonProps {
  label: string;
  ariaLabel?: string;
  onPress: () => void;
  disabled?: boolean;
}

/** A momentary hardware button: fires, springs back. No latch state, no
 * LED — use PushButton for anything that stays down. */
export function ActionButton({ label, ariaLabel, onPress, disabled = false }: ActionButtonProps) {
  return (
    <button
      type="button"
      className={styles.button}
      aria-label={ariaLabel ?? label}
      disabled={disabled}
      onClick={onPress}
    >
      {label}
    </button>
  );
}
