import { FADER_MIN_DB, formatDb } from '../../audio/db';
import { Knob } from '../../design/Knob';
import { useMixer } from '../../state/mixer';
import type { StripState } from '../../ws/messages';
import { gesture } from '../../ws/send';

/** One yellow send knob per aux bus. A send that was never touched sits at
 * the floor — the wire model has no "absent" send. */
export function AuxSends({ strip }: { strip: StripState }) {
  // Select the STORED reference and derive in render: a selector that
  // filters returns a fresh array every call, which useSyncExternalStore
  // reads as an ever-changing snapshot — an infinite re-render loop.
  const buses = useMixer((s) => s.state.buses);
  const auxBuses = buses.filter((b) => b.kind === 'aux');
  return (
    <>
      {auxBuses.map((bus, i) => {
        const existing = strip.sends.find((s) => s.dest === bus.id);
        const level = existing?.level_db ?? FADER_MIN_DB;
        const tap = existing?.tap ?? 'post_fader';
        return (
          <Knob
            key={bus.id}
            label={`Aux ${i + 1}`}
            value={level}
            min={FADER_MIN_DB}
            max={10}
            defaultValue={FADER_MIN_DB}
            step={0.5}
            bigStep={3}
            format={formatDb}
            cap="yellow"
            onChange={(level_db) =>
              gesture(
                { kind: 'send', strip: strip.id, dest: bus.id, level_db, tap },
                { op: 'set_send', strip: strip.id, dest: bus.id, level_db, tap },
              )
            }
          />
        );
      })}
    </>
  );
}
