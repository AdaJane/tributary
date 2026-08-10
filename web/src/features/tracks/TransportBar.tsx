import { $api } from '../../api/client';
import { useMonitorStream } from '../../audio/monitor/useMonitorStream';
import { ActionButton } from '../../design/ActionButton';
import { PushButton } from '../../design/PushButton';
import { usePeaksStore } from '../../state/peaks';
import { useTransport } from '../../state/transport';
import { timecode } from './timecode';
import styles from './TransportBar.module.css';

/** The tape-deck row: RTZ · PLAY · STOP · REC and the counter. One STOP
 * stops whatever runs — playback or the take. */
export function TransportBar() {
  const phase = useTransport((s) => s.phase);
  const take = useTransport((s) => s.take);
  const positionFrames = useTransport((s) => s.positionFrames);
  const sampleRate = useTransport((s) => s.sampleRate);
  const totalFrames = useTransport((s) => s.totalFrames);
  const hasLoop = useTransport((s) => s.loop !== null);
  const monitor = useTransport((s) => s.monitor);
  // Sample truth for the recording counter: frames the writer has binned.
  const liveFrames = usePeaksStore((s) => s.liveFrames);
  const { needsTap, enable } = useMonitorStream();

  const playing = phase === 'playing';
  const recording = phase === 'recording';
  const noTake = take === null;

  const counter = recording
    ? `REC ${timecode(liveFrames, sampleRate)}`
    : `${timecode(positionFrames, sampleRate)} / ${timecode(totalFrames, sampleRate)}`;

  return (
    <div className={styles.bar}>
      <ActionButton
        label="RTZ"
        ariaLabel="Return to zero"
        disabled={recording || noTake}
        onPress={() => void $api.POST('/api/v1/transport/seek', { body: { position_frames: 0 } })}
      />
      <PushButton
        label="Play"
        ariaLabel={playing ? 'Playing the latest take' : 'Play the latest take'}
        variant="pfl"
        pressed={playing}
        disabled={recording || noTake}
        onToggle={() => {
          void (playing ? $api.POST('/api/v1/transport/stop') : $api.POST('/api/v1/transport/play'));
        }}
      />
      <ActionButton
        label="Stop"
        ariaLabel="Stop playback or recording"
        disabled={phase === 'stopped'}
        onPress={() => {
          void (recording
            ? $api.POST('/api/v1/transport/record/stop')
            : $api.POST('/api/v1/transport/stop'));
        }}
      />
      <PushButton
        label="Rec"
        ariaLabel={recording ? 'Stop recording' : 'Start recording'}
        variant="arm"
        pressed={recording}
        blinking={recording}
        onToggle={() => {
          void (recording
            ? $api.POST('/api/v1/transport/record/stop')
            : $api.POST('/api/v1/transport/record/start'));
        }}
      />
      <output className={styles.counter} data-recording={recording || undefined}>
        {take !== null && `T${String(take).padStart(2, '0')} `}
        {counter}
      </output>
      {hasLoop && (
        <ActionButton
          label="Loop ✕"
          ariaLabel="Clear the loop region"
          onPress={() => void $api.DELETE('/api/v1/transport/loop')}
        />
      )}
      <PushButton
        label="Mon"
        ariaLabel={
          monitor === 'stream'
            ? 'Monitor in this browser (hardware keeps the live mix)'
            : 'Monitor on the console hardware output'
        }
        variant="plain"
        pressed={monitor === 'stream'}
        onToggle={(on) =>
          void $api.PUT('/api/v1/transport/monitor', {
            body: { target: on ? 'stream' : 'hardware' },
          })
        }
      />
      {needsTap && (
        <ActionButton label="Enable audio" ariaLabel="Enable browser audio" onPress={enable} />
      )}
    </div>
  );
}
