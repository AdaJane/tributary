import { formatDb } from '../../audio/db';
import { formatHz, hzToPosition, positionToHz } from '../../audio/freq';
import { ActionButton } from '../../design/ActionButton';
import { CollapsibleSection } from '../../design/CollapsibleSection';
import { Knob } from '../../design/Knob';
import { PushButton } from '../../design/PushButton';
import { useUi } from '../../state/ui';
import type { StripState } from '../../ws/messages';
import { gesture } from '../../ws/send';
import styles from './EqSection.module.css';

const EQ_RANGE = 15;

type Band = 'low_shelf' | 'peak' | 'high_shelf';

/** Flat response at the console's centers — must match trib-core's
 * `ChannelEq::default()` exactly. */
const FLAT_EQ: Record<Band, { freq_hz: number }> = {
  low_shelf: { freq_hz: 80 },
  peak: { freq_hz: 800 },
  high_shelf: { freq_hz: 12000 },
};
const FLAT_Q = 0.71;

export function EqSection({ strip }: { strip: StripState }) {
  const expanded = useUi((s) => s.eqExpanded[String(strip.id)] ?? false);
  const setExpanded = useUi((s) => s.setEqExpanded);

  const setBand = (band: Band, patch: { gain_db?: number; freq_hz?: number }) => {
    const slot =
      band === 'low_shelf' ? strip.eq.low : band === 'peak' ? strip.eq.mid : strip.eq.high;
    const next = {
      strip: strip.id,
      band,
      freq_hz: patch.freq_hz ?? slot.freq_hz,
      gain_db: patch.gain_db ?? slot.gain_db,
      q: slot.q,
    };
    gesture({ kind: 'eq_band', ...next }, { op: 'set_eq_band', ...next });
  };

  /** Every band back to flat; the engage switch is deliberately untouched. */
  const reset = () => {
    for (const band of ['low_shelf', 'peak', 'high_shelf'] as const) {
      const next = {
        strip: strip.id,
        band,
        freq_hz: FLAT_EQ[band].freq_hz,
        gain_db: 0,
        q: FLAT_Q,
      };
      gesture({ kind: 'eq_band', ...next }, { op: 'set_eq_band', ...next });
    }
  };

  const summary = [
    `HF ${formatDb(strip.eq.high.gain_db)}`,
    `${formatHz(strip.eq.mid.freq_hz)} ${formatDb(strip.eq.mid.gain_db)}`,
    `LF ${formatDb(strip.eq.low.gain_db)}`,
  ].join(' · ');

  return (
    <CollapsibleSection
      title="EQ"
      expanded={expanded}
      onToggle={(open) => setExpanded(String(strip.id), open)}
      summary={summary}
      engaged={strip.eq.enabled}
    >
      <div className={styles.switches}>
        {/* Status lamp is the section-header dot; no second LED here. */}
        <PushButton
          label="On"
          ariaLabel={`EQ engaged for ${strip.name}`}
          variant="plain"
          led={false}
          pressed={strip.eq.enabled}
          onToggle={(enabled) =>
            gesture(
              { kind: 'eq_enabled', strip: strip.id, enabled },
              { op: 'set_eq_enabled', strip: strip.id, enabled },
            )
          }
        />
        <ActionButton
          label="Reset"
          ariaLabel={`Reset ${strip.name} EQ to flat`}
          onPress={reset}
        />
      </div>
      <Knob
        label="HF"
        value={strip.eq.high.gain_db}
        min={-EQ_RANGE}
        max={EQ_RANGE}
        defaultValue={0}
        step={0.5}
        bigStep={3}
        format={formatDb}
        cap="blue"
        onChange={(gain_db) => setBand('high_shelf', { gain_db })}
      />
      <Knob
        label="Mid"
        value={strip.eq.mid.gain_db}
        min={-EQ_RANGE}
        max={EQ_RANGE}
        defaultValue={0}
        step={0.5}
        bigStep={3}
        format={formatDb}
        cap="blue"
        onChange={(gain_db) => setBand('peak', { gain_db })}
      />
      <Knob
        label="Freq"
        value={strip.eq.mid.freq_hz}
        min={100}
        max={8000}
        defaultValue={800}
        step={50}
        bigStep={500}
        format={formatHz}
        toPosition={hzToPosition}
        fromPosition={positionToHz}
        cap="green"
        onChange={(freq_hz) => setBand('peak', { freq_hz })}
      />
      <Knob
        label="LF"
        value={strip.eq.low.gain_db}
        min={-EQ_RANGE}
        max={EQ_RANGE}
        defaultValue={0}
        step={0.5}
        bigStep={3}
        format={formatDb}
        cap="blue"
        onChange={(gain_db) => setBand('low_shelf', { gain_db })}
      />
    </CollapsibleSection>
  );
}
