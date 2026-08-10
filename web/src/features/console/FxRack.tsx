import { FADER_MIN_DB, formatDb } from '../../audio/db';
import { Knob } from '../../design/Knob';
import { useMixer } from '../../state/mixer';
import type { MixerState } from '../../ws/messages';
import { gesture } from '../../ws/send';
import styles from './FxRack.module.css';

type FxUnit = MixerState['fx'][number];

function FxUnitPanel({ unit }: { unit: FxUnit }) {
  const setParams = (params: FxUnit['params']) =>
    gesture(
      { kind: 'fx_params', fx: unit.id, params },
      { op: 'set_fx_params', fx: unit.id, params },
    );
  return (
    <section className={styles.unit}>
      <h3 className={styles.name}>{unit.name}</h3>
      <div className={styles.knobs}>
        {unit.params.kind === 'reverb' ? (
          <>
            <Knob
              label="Room"
              value={unit.params.room_size}
              min={0}
              max={1}
              defaultValue={0.5}
              step={0.05}
              bigStep={0.2}
              format={(v) => `${Math.round(v * 100)}%`}
              cap="grey"
              onChange={(room_size) =>
                setParams({ kind: 'reverb', room_size, damping: unit.params.kind === 'reverb' ? unit.params.damping : 0.5 })
              }
            />
            <Knob
              label="Damp"
              value={unit.params.damping}
              min={0}
              max={1}
              defaultValue={0.5}
              step={0.05}
              bigStep={0.2}
              format={(v) => `${Math.round(v * 100)}%`}
              cap="grey"
              onChange={(damping) =>
                setParams({ kind: 'reverb', room_size: unit.params.kind === 'reverb' ? unit.params.room_size : 0.5, damping })
              }
            />
          </>
        ) : (
          <>
            <Knob
              label="Time"
              value={unit.params.time_ms}
              min={1}
              max={2000}
              defaultValue={350}
              step={10}
              bigStep={100}
              format={(v) => `${Math.round(v)}ms`}
              cap="grey"
              onChange={(time_ms) =>
                setParams({ kind: 'delay', time_ms, feedback: unit.params.kind === 'delay' ? unit.params.feedback : 0.35 })
              }
            />
            <Knob
              label="Fdbk"
              value={unit.params.feedback}
              min={0}
              max={0.95}
              defaultValue={0.35}
              step={0.05}
              bigStep={0.2}
              format={(v) => `${Math.round(v * 100)}%`}
              cap="grey"
              onChange={(feedback) =>
                setParams({ kind: 'delay', time_ms: unit.params.kind === 'delay' ? unit.params.time_ms : 350, feedback })
              }
            />
          </>
        )}
        <Knob
          label="Return"
          value={unit.return_level_db}
          min={FADER_MIN_DB}
          max={10}
          defaultValue={-10}
          step={0.5}
          bigStep={3}
          format={formatDb}
          cap="green"
          onChange={(level_db) =>
            gesture(
              { kind: 'fx_return', fx: unit.id, level_db },
              { op: 'set_fx_return', fx: unit.id, level_db },
            )
          }
        />
      </div>
    </section>
  );
}

/** The outboard rack: one panel per FX unit, living in the master section. */
export function FxRack() {
  const fx = useMixer((s) => s.state.fx);
  return (
    <div className={styles.rack}>
      {fx.map((unit) => (
        <FxUnitPanel key={unit.id} unit={unit} />
      ))}
    </div>
  );
}
