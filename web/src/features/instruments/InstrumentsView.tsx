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
            <ActionButton
              label="Refresh"
              ariaLabel="Re-scan MIDI inputs and retry anything that failed to load"
              onPress={() => void refresh()}
            />
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
