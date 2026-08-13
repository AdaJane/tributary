import { memo } from 'react';

import { type LedStop, belowScale, formatPeak } from '../audio/leds';
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
  // `aria-valuenow` is clamped to the scale, so on its own it reports a
  // signal 50 dB under the bottom stop as if it were sitting on it.
  // `aria-valuetext` carries the reading the LEDs quantized away, and the
  // title puts the same number a hover from anyone debugging gain staging.
  const reading = formatPeak(peakDb);
  const under = belowScale(peakDb, stops);
  const column = (
    <div
      className={styles.column}
      role="meter"
      aria-label={label}
      aria-valuemin={stops[0].db}
      aria-valuemax={0}
      aria-valuenow={Math.max(Math.round(peakDb), stops[0].db)}
      aria-valuetext={reading}
      title={reading}
      data-disabled={disabled || undefined}
      data-below-scale={under || undefined}
    >
      {stops.map((stop, i) => {
        const isTop = i === stops.length - 1;
        const isBottom = i === 0;
        return (
          <span
            key={stop.db}
            className={styles.led}
            data-color={stop.color}
            data-on={(i < lit || (isTop && clip)) || undefined}
            // Signal is arriving, just not enough to reach the first stop.
            // Distinguishes a quiet input from an unpatched one without
            // adding any chrome to a surface that has no room for it.
            data-trace={(isBottom && under) || undefined}
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
