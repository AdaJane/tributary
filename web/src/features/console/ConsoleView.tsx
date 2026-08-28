import { useEffect } from 'react';

import { $api } from '../../api/client';
import { loadAudioStatus } from '../../state/audio';
import { loadDevices, useDevices } from '../../state/devices';
import { loadOutputs } from '../../state/outputs';
import { useMixer } from '../../state/mixer';
import type { StripState } from '../../ws/messages';
import styles from './ConsoleView.module.css';
import { ChannelStrip } from './ChannelStrip';
import { groupPatchbay } from './devices';
import { inputSource, sourceKey } from './input-source';
import { deviceLetters, sharedInputGroups } from './linked-inputs';
import { MasterSection } from './MasterSection';
import { useMeterFeed } from './useMeterFeed';
import { useMixerFeed } from './useMixerFeed';
import { useTransportFeed } from './useTransportFeed';

export function ConsoleView() {
  useMixerFeed();
  useMeterFeed();
  useTransportFeed();
  const loaded = useMixer((s) => s.loaded);
  const strips = useMixer((s) => s.state.strips);
  const masterDb = useMixer((s) => s.state.master.fader_db);
  const devices = useDevices((s) => s.devices);

  // One boot-time device read so jack labels and letters can print; the
  // patchbay's open/Refresh replaces it.
  useEffect(() => {
    void loadDevices();
    // The same for outputs, so every OUT button can print its patch
    // before anyone opens the room.
    void loadOutputs();
    // And the backend itself, so a dark patch bay can say why at once.
    void loadAudioStatus();
  }, []);

  if (!loaded) {
    return (
      <div className={styles.waiting}>
        <p className={styles.tape}>Patching in…</p>
        <p className={styles.hint}>Waiting for the daemon’s mixer snapshot.</p>
      </div>
    );
  }

  const letters = deviceLetters(
    groupPatchbay(devices, strips)
      .filter((s) => s.device !== null)
      .map((s) => s.device as string),
  );
  const links = sharedInputGroups(strips, letters);
  // A strip wears link tape when something else shares whatever feeds it —
  // an instrument channel just as much as a jack.
  const linkFor = (input: StripState['input']) => {
    const source = inputSource(input);
    return source === null ? undefined : links.get(sourceKey(source));
  };

  return (
    <div className={styles.console}>
      <div className={styles.strips}>
        {strips.map((strip) => (
          <ChannelStrip
            key={strip.id}
            strip={strip}
            link={linkFor(strip.input)}
          />
        ))}
        <button
          type="button"
          className={styles.addStrip}
          aria-label="Add channel"
          onClick={() => void $api.POST('/api/v1/strips', { body: {} })}
        >
          +
        </button>
      </div>
      <MasterSection masterDb={masterDb} />
    </div>
  );
}
