import { useEffect, useState } from 'react';

import { $api } from '../../api/client';
import { formatDb } from '../../audio/db';
import { Fader } from '../../design/Fader';
import { Knob } from '../../design/Knob';
import { PushButton } from '../../design/PushButton';
import { TapeLabel } from '../../design/TapeLabel';
import { OutputButton } from './OutputButton';
import { useTransport } from '../../state/transport';
import type { StripState } from '../../ws/messages';
import { gesture } from '../../ws/send';
import { AuxSends } from './AuxSends';
import styles from './ChannelStrip.module.css';
import { EqSection } from './EqSection';
import { InputButton } from './InputButton';
import type { InputLink } from './linked-inputs';

import { StripMeter } from './StripMeter';

const formatPan = (pan: number) =>
  pan === 0 ? 'C' : pan < 0 ? `L${Math.round(-pan * 100)}` : `R${Math.round(pan * 100)}`;

/** How long an armed remove waits for its confirming second click. */
const REMOVE_CONFIRM_MS = 3000;

/** Pulling a channel is two clicks — arm, then confirm — so one stray tap
 * never rips a strip off the desk. Disabled while tape rolls (the console
 * layout is frozen during a take). */
function RemoveStripButton({ strip, recording }: { strip: StripState; recording: boolean }) {
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    if (!armed) return;
    const timer = setTimeout(() => setArmed(false), REMOVE_CONFIRM_MS);
    return () => clearTimeout(timer);
  }, [armed]);

  return (
    <button
      type="button"
      className={styles.remove}
      data-armed={armed || undefined}
      disabled={recording}
      aria-label={
        recording
          ? `Remove ${strip.name} (stop recording first)`
          : armed
            ? `Confirm removing ${strip.name}`
            : `Remove channel ${strip.name}`
      }
      onClick={() => {
        if (!armed) {
          setArmed(true);
          return;
        }
        void $api.DELETE('/api/v1/strips/{id}', {
          params: { path: { id: strip.id } },
        });
      }}
    >
      {armed ? 'SURE?' : '✕'}
    </button>
  );
}

export function ChannelStrip({ strip, link }: { strip: StripState; link?: InputLink }) {
  const target = { kind: 'strip', id: strip.id } as const;
  const recording = useTransport((s) => s.phase === 'recording');
  return (
    <div className={styles.strip} data-muted={strip.mute || undefined}>
      {/* A shared input wears matching colored tape on every strip it
          feeds — the patchbay link, visible at a glance. */}
      {link && (
        <div
          className={styles.linkTape}
          data-cap={link.color}
          role="img"
          aria-label={`Input ${link.label} shared with ${
            link.stripIds.length - 1
          } other channel${link.stripIds.length > 2 ? 's' : ''}`}
        >
          {link.label}
        </div>
      )}
      <RemoveStripButton strip={strip} recording={recording} />
      <InputButton strip={strip} />
      <Knob
        label="Gain"
        value={strip.gain_db}
        min={-20}
        max={60}
        defaultValue={0}
        step={0.5}
        bigStep={5}
        format={formatDb}
        cap="red"
        onChange={(gain_db) =>
          gesture(
            { kind: 'gain', strip: strip.id, gain_db },
            { op: 'set_gain', strip: strip.id, gain_db },
          )
        }
      />
      <EqSection strip={strip} />
      <AuxSends strip={strip} />
      <Knob
        label="Pan"
        value={strip.pan}
        min={-1}
        max={1}
        defaultValue={0}
        step={0.05}
        bigStep={0.25}
        format={formatPan}
        cap="white"
        onChange={(pan) =>
          gesture(
            { kind: 'pan', strip: strip.id, pan },
            { op: 'set_pan', strip: strip.id, pan },
          )
        }
      />
      <div className={styles.meterRow}>
        <StripMeter meterKey={`strip:${strip.id}`} label={`${strip.name} level`} />
        <Fader
          label={`${strip.name} fader`}
          value={strip.fader_db}
          cap="white"
          onChange={(level_db) =>
            gesture(
              { kind: 'fader', target, level_db },
              { op: 'set_fader', target, level_db },
            )
          }
        />
      </div>
      <div className={styles.switches}>
        <PushButton
          label="PFL"
          ariaLabel={`PFL ${strip.name}`}
          variant="pfl"
          pressed={strip.pfl}
          onToggle={(on) =>
            gesture({ kind: 'pfl', target, on }, { op: 'set_pfl', target, on })
          }
        />
        <PushButton
          label="Mute"
          ariaLabel={`Mute ${strip.name}`}
          variant="mute"
          pressed={strip.mute}
          onToggle={(mute) =>
            gesture({ kind: 'mute', target, mute }, { op: 'set_mute', target, mute })
          }
        />
        <PushButton
          label="Arm"
          ariaLabel={`Arm ${strip.name} for recording`}
          variant="arm"
          pressed={strip.record_arm}
          blinking={recording}
          onToggle={(armed) =>
            gesture(
              { kind: 'record_arm', target, armed },
              { op: 'set_record_arm', target, armed },
            )
          }
        />
      </div>
      <OutputButton source={{ kind: 'strip', id: strip.id }} name={strip.name} />
      <TapeLabel
        id={`strip-${strip.id}`}
        name={strip.name}
        onRename={(name) =>
          gesture(
            { kind: 'renamed', target, name },
            { op: 'rename', target, name },
          )
        }
      />
    </div>
  );
}
