import { memo } from 'react';

import type { LedStop } from '../audio/leds';
import styles from './LedMeter.module.css';

export interface LedMeterProps {
  /** How many LEDs are on, bottom-up (from `litSegments`). */
  lit: number;
  clip: boolean;
  stops: readonly LedStop[];
  label: string;
  /** For aria-valuenow; the LEDs themselves are quantized. */
  peakDb: number;
  /** When clipped, the meter becomes a button that clears the hold. */
  onClearClip?: () => void;
  disabled?: boolean;
}

/** LED state changes deliberately never transition — hardware snaps. */
export const LedMeter = memo(function LedMeter({
  lit,
  clip,
  stops,
  label,
  peakDb,
  onClearClip,
  disabled = false,
}: LedMeterProps) {
  const column = (
    <div
      className={styles.column}
      role="meter"
      aria-label={label}
      aria-valuemin={stops[0].db}
      aria-valuemax={0}
      aria-valuenow={Math.max(Math.round(peakDb), stops[0].db)}
      data-disabled={disabled || undefined}
    >
      {stops.map((stop, i) => {
        const isTop = i === stops.length - 1;
        return (
          <span
            key={stop.db}
            className={styles.led}
            data-color={stop.color}
            data-on={(i < lit || (isTop && clip)) || undefined}
          />
        );
      })}
    </div>
  );
  return clip && onClearClip ? (
    <button
      type="button"
      className={styles.clipClear}
      aria-label={`${label}: clipped — clear`}
      onClick={onClearClip}
    >
      {column}
    </button>
  ) : (
    column
  );
});
