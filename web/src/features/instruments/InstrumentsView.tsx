import { useEffect } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { InlineError, Panel, Waiting } from '../../design/Panel';
import { useInstruments } from '../../state/instruments';
import { useMixer } from '../../state/mixer';
import { useTransport } from '../../state/transport';
import { InstrumentRack } from './InstrumentRack';
import { MidiInputsPanel } from './MidiInputsPanel';
import { SoundfontSection } from './SoundfontSection';
import styles from './InstrumentsView.module.css';

/**
 * The instrument rack: what the box plays when nothing is plugged into it.
 *
 * Panel order is frequency first, like Setup putting Session at the top —
 * and it gives the page a downward repair gradient. A rack unit that says
 * "no soundfont chosen" sends you to the panel below it; one that says
 * "nothing arriving" sends you to the one below that. Reading order is
 * repair order.
 */
export function InstrumentsView() {
  const doc = useInstruments((s) => s.doc);
  const loaded = useInstruments((s) => s.loaded);
  const error = useInstruments((s) => s.error);
  const load = useInstruments((s) => s.load);
  const refresh = useInstruments((s) => s.refresh);
  const panic = useInstruments((s) => s.panic);
  const phase = useTransport((s) => s.phase);
  const recording = phase === 'recording';
  const stripCount = useMixer((s) => s.state.strips.length);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className={styles.page}>
      <div className={styles.column}>
        <Panel
          title="Instrument rack"
          badge={
            <>
              <ActionButton
                label="Refresh"
                ariaLabel="Re-scan MIDI inputs and retry anything that failed to load"
                onPress={() => void refresh()}
              />
              {/* One panic, both directions. A stuck note on an external
                  synth is now this box's fault as much as an internal one,
                  and a second button would leave the user guessing which
                  half a hanging note came from. */}
              <ActionButton
                label="Panic"
                ariaLabel="Silence every instrument, and send sustain-off then all-notes-off to every MIDI output"
                onPress={() => void panic()}
              />
            </>
          }
        >
          {!loaded || doc === null ? (
            <Waiting />
          ) : (
            <InstrumentRack doc={doc} recording={recording} stripCount={stripCount} />
          )}
          {error && error.id === null && <InlineError message={error.message} />}
        </Panel>

        <Panel title="MIDI inputs">
          {!loaded || doc === null ? <Waiting /> : <MidiInputsPanel doc={doc} />}
        </Panel>

        <Panel title="Soundfonts">
          {!loaded || doc === null ? (
            <Waiting />
          ) : (
            <SoundfontSection doc={doc} recording={recording} />
          )}
        </Panel>
      </div>
    </div>
  );
}
