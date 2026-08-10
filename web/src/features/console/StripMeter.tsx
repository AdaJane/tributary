import { CHANNEL_LED_STOPS, litSegments } from '../../audio/leds';
import { LedMeter } from '../../design/LedMeter';
import { SILENT_READING, readingClip, useMeters } from '../../state/meters';

/** Quantizing subscription: this component re-renders only when the LED
 * count or clip state changes, not on every 20 Hz frame. */
export function StripMeter({ meterKey, label }: { meterKey: string; label: string }) {
  const lit = useMeters((s) =>
    litSegments((s.byKey[meterKey] ?? SILENT_READING).peakDb, CHANNEL_LED_STOPS),
  );
  const clip = useMeters((s) =>
    readingClip(s.byKey[meterKey] ?? SILENT_READING, Date.now()),
  );
  const peakDb = useMeters((s) => (s.byKey[meterKey] ?? SILENT_READING).peakDb);
  const clearClip = useMeters((s) => s.clearClip);
  return (
    <LedMeter
      lit={lit}
      clip={clip}
      stops={CHANNEL_LED_STOPS}
      label={label}
      peakDb={peakDb}
      onClearClip={() => clearClip(meterKey)}
    />
  );
}
