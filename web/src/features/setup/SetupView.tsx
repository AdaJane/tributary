import { useEffect } from 'react';
import type { ReactNode } from 'react';

import { SegmentedControl } from '../../design/SegmentedControl';
import type { SegmentedOption } from '../../design/SegmentedControl';
import { loadDestinations, loadSettings, saveSettings, useSettings } from '../../state/settings';
import type { RecordingFormat } from '../../state/settings';
import { useTransport } from '../../state/transport';
import { DestinationSection } from './DestinationSection';
import { needsRestart } from './settings-logic';
import styles from './SetupView.module.css';

const FORMAT_OPTIONS: readonly SegmentedOption<RecordingFormat>[] = [
  { value: 'wav16', label: 'WAV 16' },
  { value: 'wav24', label: 'WAV 24' },
  { value: 'wav32_float', label: 'WAV 32F' },
  { value: 'flac16', label: 'FLAC 16' },
  { value: 'flac24', label: 'FLAC 24' },
];

type RateValue = '44100' | '48000' | '96000';

const RATE_OPTIONS: readonly SegmentedOption<RateValue>[] = [
  { value: '44100', label: '44.1 kHz' },
  { value: '48000', label: '48 kHz' },
  { value: '96000', label: '96 kHz' },
];

function Panel({
  title,
  badge,
  children,
}: {
  title: string;
  badge?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className={styles.panel}>
      <header className={styles.panelHeader}>
        <h2 className={styles.panelTitle}>{title}</h2>
        {badge}
      </header>
      {children}
    </section>
  );
}

function InlineError({ message }: { message: string }) {
  return (
    <p className={styles.error} role="status">
      <span className={styles.errorDot} />
      {message}
    </p>
  );
}

/**
 * The console's rear panel: recording destination, file format, and
 * sample rate. Every control applies immediately (the app-wide idiom);
 * refused saves roll back with an inline lamp-and-line.
 */
export function SetupView() {
  const settings = useSettings((s) => s.settings);
  const error = useSettings((s) => s.error);
  const phase = useTransport((s) => s.phase);
  const engineRate = useTransport((s) => s.sampleRate);
  const recordingNow = phase === 'recording';

  useEffect(() => {
    void loadSettings();
    void loadDestinations();
  }, []);

  const restart =
    settings !== null &&
    needsRestart(settings.configuredSampleRate, engineRate, settings.restartRequired);

  return (
    <div className={styles.page}>
      <div className={styles.column}>
        <Panel title="Destination">
          <DestinationSection locked={recordingNow} />
        </Panel>

        <Panel title="File format">
          {settings === null ? (
            <p className={styles.waiting}>waiting for the daemon…</p>
          ) : (
            <>
              <SegmentedControl
                label="File format"
                options={FORMAT_OPTIONS}
                value={settings.format}
                onChange={(format) => void saveSettings({ format }, 'format')}
              />
              <p className={styles.note}>applies to the next recording</p>
              {error?.field === 'format' && <InlineError message={error.message} />}
            </>
          )}
        </Panel>

        <Panel
          title="Sample rate"
          badge={
            restart ? (
              <span className={styles.restart} role="status">
                <span className={styles.restartDot} />
                Restart required
              </span>
            ) : undefined
          }
        >
          {settings === null ? (
            <p className={styles.waiting}>waiting for the daemon…</p>
          ) : (
            <>
              <SegmentedControl
                label="Sample rate"
                options={RATE_OPTIONS}
                value={String(settings.configuredSampleRate) as RateValue}
                onChange={(rate) => void saveSettings({ sampleRate: Number(rate) }, 'sampleRate')}
              />
              <p className={styles.engine}>
                <span className={styles.engineLabel}>Engine</span>
                <span className={styles.engineValue}>
                  {(settings.activeSampleRate / 1000).toLocaleString()} kHz
                </span>
              </p>
              <p className={styles.note}>the engine rate changes at the next daemon start</p>
              {error?.field === 'sampleRate' && <InlineError message={error.message} />}
            </>
          )}
        </Panel>
      </div>
    </div>
  );
}
