import { MASTER_LED_STOPS, litSegments } from '../../audio/leds';
import { Fader } from '../../design/Fader';
import { LedMeter } from '../../design/LedMeter';
import { TapeLabel } from '../../design/TapeLabel';
import { renameSession, useSessions } from '../../state/sessions';
import { SILENT_READING, readingClip, useMeters } from '../../state/meters';
import { gesture } from '../../ws/send';
import { FxRack } from './FxRack';
import styles from './MasterSection.module.css';
import { Transport } from './Transport';

export function MasterSection({ masterDb }: { masterDb: number }) {
  const lit = useMeters((s) =>
    litSegments((s.byKey['master'] ?? SILENT_READING).peakDb, MASTER_LED_STOPS),
  );
  const clip = useMeters((s) =>
    readingClip(s.byKey['master'] ?? SILENT_READING, Date.now()),
  );
  const peakDb = useMeters((s) => (s.byKey['master'] ?? SILENT_READING).peakDb);
  const clearClip = useMeters((s) => s.clearClip);
  const session = useSessions((s) => s.sessions.find((row) => row.open) ?? null);
  const target = { kind: 'master' } as const;

  return (
    <aside className={styles.master}>
      {/* The open session's own name, and the rename affordance the
          console uses everywhere else. This was hardcoded to "Session"
          while the daemon had been sending the real name all along —
          after opening a differently-named session the tape simply lied. */}
      <TapeLabel
        id="session"
        name={session?.name ?? 'Session'}
        onRename={session ? (name) => void renameSession(session.id, name) : undefined}
      />
      <div className={styles.meters}>
        {/* One master reading feeds both columns until stereo metering
            lands with the bus work. */}
        <LedMeter lit={lit} clip={clip} stops={MASTER_LED_STOPS} label="Master level L" peakDb={peakDb} onClearClip={() => clearClip('master')} />
        <LedMeter lit={lit} clip={clip} stops={MASTER_LED_STOPS} label="Master level R" peakDb={peakDb} />
      </div>
      {clip && <span className={styles.clipText}>CLIP</span>}
      <Fader
        label="Master fader"
        value={masterDb}
        cap="red"
        onChange={(level_db) =>
          gesture(
            { kind: 'fader', target, level_db },
            { op: 'set_fader', target, level_db },
          )
        }
      />
      <FxRack />
      <Transport />
    </aside>
  );
}
