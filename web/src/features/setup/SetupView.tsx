import { useEffect } from 'react';

import { InlineError, Panel, Waiting } from '../../design/Panel';
import { SegmentedControl } from '../../design/SegmentedControl';
import type { SegmentedOption } from '../../design/SegmentedControl';
import { loadSettings, saveSettings, useSettings } from '../../state/settings';
import { useDestinationsFeed } from './useDestinationsFeed';
import type { RecordingFormat } from '../../state/settings';
import { useTransport } from '../../state/transport';
import { DestinationSection } from './DestinationSection';
import { SessionSection } from './SessionSection';
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
  }, []);
  // Drives arrive on their own from here — the daemon pushes on every
  // mount-table change, so plugging one in is not a race against Rescan.
  useDestinationsFeed();

  const restart =
    settings !== null &&
    needsRestart(settings.configuredSampleRate, engineRate, settings.restartRequired);

  return (
    <div className={styles.page}>
      <div className={styles.column}>
        {/* Sessions first: which reel of tape is on the machine is the
            more frequent choice, and the destination is where they live. */}
        <Panel title="Session">
          <SessionSection locked={recordingNow} />
        </Panel>

        <Panel title="Destination">
          <DestinationSection locked={recordingNow} />
        </Panel>

        <Panel title="File format">
          {settings === null ? (
            <Waiting />
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
            <Waiting />
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
