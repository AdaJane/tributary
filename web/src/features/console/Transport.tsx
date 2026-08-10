import { useEffect, useState } from 'react';

import { $api } from '../../api/client';
import { PushButton } from '../../design/PushButton';
import { useMixer } from '../../state/mixer';
import { useTransport } from '../../state/transport';
import { gesture } from '../../ws/send';
import styles from './Transport.module.css';

function elapsedLabel(startedAtUnix: number | null): string {
  if (startedAtUnix === null) return '--:--';
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - startedAtUnix);
  const mm = String(Math.floor(seconds / 60)).padStart(2, '0');
  const ss = String(seconds % 60).padStart(2, '0');
  return `${mm}:${ss}`;
}

export function Transport() {
  // Primitives only: positionFrames churns at 20 Hz while playing, and this
  // panel must not repaint with it.
  const recording = useTransport((s) => s.phase === 'recording');
  const take = useTransport((s) => s.take);
  const startedAtUnix = useTransport((s) => s.startedAtUnix);
  const allArmed = useMixer(
    (s) => s.state.strips.length > 0 && s.state.strips.every((strip) => strip.record_arm),
  );
  const masterArmed = useMixer((s) => s.state.master.record_arm);
  // A 1 Hz repaint while the tape rolls, for the elapsed readout.
  const [, tick] = useState(0);
  useEffect(() => {
    if (!recording) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [recording]);

  return (
    <div className={styles.transport}>
      <div className={styles.row}>
        <PushButton
          label="Arm all"
          ariaLabel="Arm every channel for recording"
          variant="arm"
          pressed={allArmed}
          onToggle={(armed) =>
            gesture({ kind: 'record_arm_all', armed }, { op: 'set_record_arm_all', armed })
          }
        />
        <PushButton
          label="Mix"
          ariaLabel="Arm the master mix for recording"
          variant="arm"
          pressed={masterArmed}
          blinking={recording}
          onToggle={(armed) =>
            gesture(
              { kind: 'record_arm', target: { kind: 'master' }, armed },
              { op: 'set_record_arm', target: { kind: 'master' }, armed },
            )
          }
        />
      </div>
      <div className={styles.row}>
        <PushButton
          label={recording ? 'Stop' : 'Rec'}
          ariaLabel={recording ? 'Stop recording' : 'Start recording'}
          variant={recording ? 'plain' : 'mute'}
          pressed={recording}
          onToggle={() => {
            void (recording
              ? $api.POST('/api/v1/transport/record/stop')
              : $api.POST('/api/v1/transport/record/start'));
          }}
        />
        <output className={styles.clock} data-recording={recording || undefined}>
          {recording && take !== null
            ? `T${String(take).padStart(2, '0')} ${elapsedLabel(startedAtUnix)}`
            : 'STOPPED'}
        </output>
      </div>
    </div>
  );
}
