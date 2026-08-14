import type { InstrumentsDocument } from '../../state/instruments';
import styles from './InstrumentsView.module.css';

/**
 * What MIDI the box can see, and which of it anything is listening to.
 *
 * A port an instrument names but the system does not offer still gets a
 * row — "not connected" has to be visible somewhere, or an instrument that
 * makes no sound has no story anywhere in the console.
 */
export function MidiInputsPanel({ doc }: { doc: InstrumentsDocument }) {
  if (doc.midiPorts.length === 0) {
    return (
      <p className={styles.empty}>
        No MIDI device connected — plug one in and press Refresh.
      </p>
    );
  }
  return (
    <ul className={styles.ports}>
      {doc.midiPorts.map((port) => (
        <li key={port.id} className={styles.port}>
          <span className={styles.portName}>{port.name}</span>
          <span
            className={styles.status}
            data-status={port.absent ? 'missing' : port.connected ? 'live' : 'ready'}
          >
            <span className={styles.statusDot} aria-hidden />
            {port.absent ? 'missing' : port.connected ? 'live' : 'ready'}
          </span>
          <span className={styles.hint}>
            {port.absent
              ? 'an instrument wants this input, but it is not connected'
              : port.connected
                ? 'an instrument is listening to it'
                : 'no instrument is listening to this input'}
          </span>
        </li>
      ))}
    </ul>
  );
}
