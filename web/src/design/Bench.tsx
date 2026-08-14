/**
 * DEV-only test bench: every control in every state, with live local state,
 * for eyeball QA against MASTER.md. Never ships — the view registry gates
 * it behind import.meta.env.DEV.
 */
import { useState } from 'react';

import { FADER_MIN_DB, formatDb } from '../audio/db';
import { CHANNEL_LED_STOPS, MASTER_LED_STOPS, litSegments } from '../audio/leds';
import { formatHz, hzToPosition, positionToHz } from '../audio/freq';
import styles from './Bench.module.css';
import { CollapsibleSection } from './CollapsibleSection';
import { Fader } from './Fader';
import { Knob } from './Knob';
import { LedMeter } from './LedMeter';
import { PushButton } from './PushButton';
import { SelectField } from './SelectField';
import { TapeLabel } from './TapeLabel';

export function Bench() {
  const [gain, setGain] = useState(0);
  const [freq, setFreq] = useState(800);
  const [pan, setPan] = useState(0);
  const [fader, setFader] = useState(FADER_MIN_DB);
  const [meterDb, setMeterDb] = useState(-20);
  const [clip, setClip] = useState(false);
  const [mute, setMute] = useState(false);
  const [pfl, setPfl] = useState(false);
  const [arm, setArm] = useState(true);
  const [recording, setRecording] = useState(false);
  const [eqOpen, setEqOpen] = useState(false);
  const [name, setName] = useState('Kick');

  return (
    <div className={styles.bench}>
      <section className={styles.group}>
        <h2 className={styles.title}>Knobs</h2>
        <div className={styles.row}>
          <Knob label="Gain" value={gain} min={-20} max={60} defaultValue={0} step={0.5} bigStep={5} format={formatDb} cap="red" onChange={setGain} />
          <Knob
            label="Mid"
            value={freq}
            min={100}
            max={8000}
            defaultValue={800}
            step={50}
            bigStep={500}
            format={formatHz}
            toPosition={hzToPosition}
            fromPosition={positionToHz}
            cap="green"
            onChange={setFreq}
          />
          <Knob label="Pan" value={pan} min={-1} max={1} defaultValue={0} step={0.05} bigStep={0.25} format={(v) => v.toFixed(2)} cap="white" onChange={setPan} />
          <Knob label="Aux 1" value={-12} min={-90} max={10} defaultValue={-90} format={formatDb} cap="yellow" onChange={() => {}} disabled />
        </div>
      </section>

      <section className={styles.group}>
        <h2 className={styles.title}>Fader + meters</h2>
        <div className={styles.row}>
          <Fader label="Bench fader" value={fader} onChange={setFader} cap="white" />
          <LedMeter
            lit={litSegments(meterDb, CHANNEL_LED_STOPS)}
            clip={clip}
            stops={CHANNEL_LED_STOPS}
            label="Channel meter"
            peakDb={meterDb}
            onClearClip={() => setClip(false)}
          />
          <LedMeter lit={litSegments(meterDb, MASTER_LED_STOPS)} clip={clip} stops={MASTER_LED_STOPS} label="Master meter" peakDb={meterDb} />
          <div className={styles.meterDrive}>
            <input
              type="range"
              min={-60}
              max={0}
              value={meterDb}
              aria-label="Drive meters"
              onChange={(e) => {
                const db = Number(e.target.value);
                setMeterDb(db);
                if (db >= -0.5) setClip(true);
              }}
            />
            <span>{formatDb(meterDb)}</span>
          </div>
        </div>
      </section>

      <section className={styles.group}>
        <h2 className={styles.title}>Buttons</h2>
        <div className={styles.row}>
          <PushButton label="Mute" variant="mute" pressed={mute} onToggle={setMute} />
          <PushButton label="PFL" variant="pfl" pressed={pfl} onToggle={setPfl} />
          <PushButton label="Arm" variant="arm" pressed={arm} onToggle={setArm} blinking={recording} />
          <PushButton label="Rec" variant="plain" pressed={recording} onToggle={setRecording} />
          <PushButton label="Off" variant="mute" pressed={false} onToggle={() => {}} disabled />
        </div>
      </section>

      <section className={styles.group}>
        <h2 className={styles.title}>SelectField</h2>
        <div className={styles.rowNarrow}>
          <SelectField
            label="Source"
            value="master#0"
            options={[
              { value: '', label: '— none —' },
              { value: 'master#0', label: 'Master L' },
              { value: 'master#1', label: 'Master R' },
            ]}
            onChange={() => {}}
            hint="8 out"
          />
          <SelectField
            label="Port"
            value="Scarlett MIDI"
            options={[{ value: 'nanoKEY2', label: 'nanoKEY2' }]}
            onChange={() => {}}
          />
          <SelectField
            label="Locked"
            value="master#0"
            options={[{ value: 'master#0', label: 'Master L' }]}
            onChange={() => {}}
            disabled
          />
        </div>
      </section>

      <section className={styles.group}>
        <h2 className={styles.title}>Tape + sections</h2>
        <div className={styles.rowNarrow}>
          <TapeLabel id="bench-strip" name={name} onRename={setName} />
          <CollapsibleSection title="EQ" expanded={eqOpen} onToggle={setEqOpen} summary="HF +2 · 1.2k +4 · LF 0" engaged>
            <Knob label="HF" value={2} min={-15} max={15} defaultValue={0} step={0.5} format={formatDb} cap="blue" onChange={() => {}} />
            <Knob label="LF" value={0} min={-15} max={15} defaultValue={0} step={0.5} format={formatDb} cap="blue" onChange={() => {}} />
          </CollapsibleSection>
        </div>
      </section>
    </div>
  );
}
