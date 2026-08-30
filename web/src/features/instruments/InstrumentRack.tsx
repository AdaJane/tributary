import { useEffect, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { InlineError } from '../../design/Panel';
import { SegmentedControl } from '../../design/SegmentedControl';
import { TapeLabel } from '../../design/TapeLabel';
import { MAX_STRIPS } from '../../state/limits';
import { fetchPresets, useInstruments } from '../../state/instruments';
import type { InstrumentsDocument } from '../../state/instruments';
import { useMixer } from '../../state/mixer';
import { addStripsBlocked, channelNames, feedsLabel, silenceReason, voiceBlocked } from '../console/instrument-logic';
import { inputSource } from '../console/input-source';
import { SelectField } from '../../design/SelectField';
import type { Preset } from './preset-options';
import {
  parsePresetValue,
  presetFallback,
  presetOptions,
  presetValue,
} from './preset-options';
import styles from './InstrumentsView.module.css';

const POLYPHONY_OPTIONS = [
  { value: '32', label: '32' },
  { value: '64', label: '64' },
  { value: '128', label: '128' },
  { value: '256', label: '256' },
] as const;

const OMNI = 'omni';

/**
 * One rack unit per instrument, plus the two buttons that do the setup.
 *
 * Every control applies immediately and awaits the daemon — the app-wide
 * idiom. What differs from Setup is that a refusal is scoped to the unit
 * that caused it: two instruments failing for different reasons must not
 * print into each other.
 */
export function InstrumentRack({
  doc,
  recording,
  stripCount,
}: {
  doc: InstrumentsDocument;
  recording: boolean;
  stripCount: number;
}) {
  const add = useInstruments((s) => s.add);
  const error = useInstruments((s) => s.error);
  const strips = useMixer((s) => s.state.strips);
  const sources = strips.map((s) => inputSource(s.input));

  const addBlocked = recording
    ? 'stop recording first'
    : doc.soundfonts.length === 0
      ? 'load a soundfont first'
      : null;

  return (
    <>
      <div className={styles.rackHead}>
        <ActionButton
          label="Add instrument"
          onPress={() => void add()}
          disabled={addBlocked !== null}
        />
        {addBlocked && <span className={styles.blocked}>{addBlocked}</span>}
      </div>

      {doc.instruments.length === 0 && (
        <p className={styles.empty}>
          {doc.soundfonts.length === 0
            ? 'No soundfonts on this appliance — load one below before adding an instrument.'
            : 'No instruments yet — add one above.'}
        </p>
      )}

      {doc.instruments.map((instrument) => {
        const report = doc.reports.find((r) => r.id === instrument.id);
        const silence = silenceReason(instrument, report);
        const blockedStrips = addStripsBlocked(instrument, recording, stripCount, MAX_STRIPS);
        return (
          <RackUnit
            key={instrument.id}
            instrument={instrument}
            doc={doc}
            report={report}
            silence={silence}
            recording={recording}
            blockedStrips={blockedStrips}
            feeds={feedsLabel(instrument, strips, sources)}
            error={error?.id === instrument.id ? error.message : null}
          />
        );
      })}
    </>
  );
}

function RackUnit({
  instrument,
  doc,
  report,
  silence,
  recording,
  blockedStrips,
  feeds,
  error,
}: {
  instrument: InstrumentsDocument['instruments'][number];
  doc: InstrumentsDocument;
  report: InstrumentsDocument['reports'][number] | undefined;
  silence: string | null;
  recording: boolean;
  blockedStrips: string | null;
  feeds: string;
  error: string | null;
}) {
  const update = useInstruments((s) => s.update);
  const setOutputs = useInstruments((s) => s.setOutputs);
  const addStrips = useInstruments((s) => s.addStrips);
  const remove = useInstruments((s) => s.remove);
  const test = useInstruments((s) => s.test);
  const [confirmRemove, setConfirmRemove] = useState(false);
  // Fetched per soundfont, not per render: a General MIDI bank holds ~300
  // presets and reading them means parsing a file that can be 206 MB. The
  // list is cleared the moment the soundfont changes so the picker never
  // shows one bank's names while another is loaded — it falls back to the
  // raw numbers, which are at least true.
  const [presets, setPresets] = useState<Preset[]>([]);
  const soundfont = instrument.soundfont;
  useEffect(() => {
    if (!soundfont) {
      setPresets([]);
      return;
    }
    let live = true;
    setPresets([]);
    void fetchPresets(soundfont).then((list) => {
      if (live) setPresets(list);
    });
    return () => {
      live = false;
    };
  }, [soundfont]);

  const voiceLock = voiceBlocked(recording);
  const splits = instrument.splits ?? [];
  const names = channelNames(instrument);

  return (
    <section className={styles.unit} aria-label={`Instrument ${instrument.name}`}>
      <div className={styles.unitHead}>
        <TapeLabel
          id={`instrument-${instrument.id}`}
          name={instrument.name}
          onRename={(name) => void update(instrument.id, { name })}
        />
        <span className={styles.slot}>INST {instrument.id + 1}</span>
        <span className={styles.status} data-status={report?.status ?? 'ready'}>
          <span className={styles.statusDot} aria-hidden />
          {report?.status ?? 'ready'}
        </span>
        <button
          type="button"
          className={styles.remove}
          aria-label={
            confirmRemove ? `Confirm removing ${instrument.name}` : `Remove ${instrument.name}`
          }
          onClick={() => {
            if (confirmRemove) {
              void remove(instrument.id);
              setConfirmRemove(false);
            } else {
              setConfirmRemove(true);
              // An instrument costs nothing to rebuild, so two clicks is
              // the right gate — the typed word is for recordings.
              window.setTimeout(() => setConfirmRemove(false), 3000);
            }
          }}
        >
          {confirmRemove ? 'SURE?' : '✕'}
        </button>
      </div>

      <Row label="Soundfont" blocked={voiceLock}>
        <select
          className={styles.select}
          aria-label={`Soundfont for ${instrument.name}`}
          value={instrument.soundfont ?? ''}
          disabled={voiceLock !== null}
          onChange={(e) =>
            void update(instrument.id, { soundfont: e.target.value || null })
          }
        >
          <option value="">— none —</option>
          {doc.soundfonts.map((sf) => (
            <option key={sf.id} value={sf.id}>
              {sf.id}
              {sf.origin === 'removable' ? ` (on ${sf.volume ?? 'a drive'})` : ''}
            </option>
          ))}
          {/* A value the list does not contain would render blank and lie
              about what is loaded. */}
          {instrument.soundfont &&
            !doc.soundfonts.some((sf) => sf.id === instrument.soundfont) && (
              <option value={instrument.soundfont}>
                {instrument.soundfont} (not connected)
              </option>
            )}
        </select>
      </Row>

      {/* A bare number input was unanswerable once a General MIDI bank
          shipped: three of them are bundled, each with ~300 sounds, and
          "program 40" tells you nothing about whether you are about to
          hear a violin. MASTER.md already specified SelectField for
          exactly this list. Bank becomes reachable at the same time —
          it had no control at all before. */}
      <Row label="Preset">
        <SelectField
          label={`Preset for ${instrument.name}`}
          value={presetValue(instrument.bank, instrument.program)}
          options={
            presets.length > 0
              ? presetOptions(presets)
              : presetFallback(instrument.bank, instrument.program)
          }
          onChange={(value) => {
            const picked = parsePresetValue(value);
            if (picked) void update(instrument.id, picked);
          }}
          hint={
            report?.presets != null ? `${report.presets} in this soundfont` : undefined
          }
        />
      </Row>

      <Row label="MIDI in">
        <select
          className={styles.select}
          aria-label={`MIDI input for ${instrument.name}`}
          value={instrument.port ?? ''}
          onChange={(e) => void update(instrument.id, { port: e.target.value || null })}
        >
          <option value="">— none —</option>
          {doc.midiPorts.map((port) => (
            <option key={port.id} value={port.id}>
              {port.name}
              {port.absent ? ' (not connected)' : ''}
            </option>
          ))}
        </select>
        <select
          className={styles.select}
          aria-label={`MIDI channel for ${instrument.name}`}
          value={instrument.midi_channel == null ? OMNI : String(instrument.midi_channel)}
          onChange={(e) =>
            void update(instrument.id, {
              midi_channel: e.target.value === OMNI ? null : Number(e.target.value),
            })
          }
        >
          <option value={OMNI}>Omni (all channels)</option>
          {Array.from({ length: 16 }, (_, i) => (
            <option key={i} value={String(i)}>
              Ch {i + 1}
            </option>
          ))}
        </select>
      </Row>

      <Row label="Outputs" blocked={voiceLock}>
        <SegmentedControl
          label={`Outputs for ${instrument.name}`}
          options={[
            { value: 'stereo', label: 'Stereo mix' },
            { value: 'drums', label: 'Drum splits' },
          ]}
          value={splits.length === 0 ? 'stereo' : 'drums'}
          disabled={voiceLock !== null}
          onChange={(value) =>
            void setOutputs(
              instrument.id,
              value === 'stereo' ? { kind: 'stereo_mix' } : { kind: 'gm_drums' },
            )
          }
        />
        <span className={styles.hint}>
          {splits.length === 0
            ? 'one stereo pair — two channels on the desk'
            : `${splits.length} mono channels: ${splits.map((s) => s.name).join(', ')}`}
        </span>
      </Row>

      <Row label="Voices" blocked={voiceLock}>
        <SegmentedControl
          label={`Polyphony for ${instrument.name}`}
          options={POLYPHONY_OPTIONS}
          value={String(instrument.polyphony) as '32' | '64' | '128' | '256'}
          disabled={voiceLock !== null}
          onChange={(value) => void update(instrument.id, { polyphony: Number(value) })}
        />
        <span className={styles.hint}>extra notes steal the oldest voice</span>
      </Row>

      <div className={styles.unitActions}>
        <ActionButton
          label="Add all channels to mixer"
          ariaLabel={`Add a channel strip for each of ${instrument.name}'s ${names.length} outputs`}
          onPress={() => void addStrips(instrument.id)}
          disabled={blockedStrips !== null}
        />
        {blockedStrips && <span className={styles.blocked}>{blockedStrips}</span>}
        <ActionButton
          label="Test note"
          ariaLabel={`Play one note on ${instrument.name}`}
          onPress={() => void test(instrument.id)}
        />
      </div>

      <p className={styles.feeds}>{feeds}</p>
      {silence && <p className={styles.silence}>Silent: {silence}</p>}
      {error && <InlineError message={error} />}
    </section>
  );
}

function Row({
  label,
  blocked,
  children,
}: {
  label: string;
  blocked?: string | null;
  children: React.ReactNode;
}) {
  return (
    <div className={styles.row}>
      <span className={styles.rowLabel}>{label}</span>
      <div className={styles.rowBody}>{children}</div>
      {/* A blocked control keeps its reason beside it — never hidden. */}
      {blocked && <span className={styles.blocked}>{blocked}</span>}
    </div>
  );
}
