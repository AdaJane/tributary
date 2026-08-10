import { useEffect } from 'react';

import { $api } from '../../api/client';
import { loadDevices, useDevices } from '../../state/devices';
import { useMixer } from '../../state/mixer';
import styles from './ConsoleView.module.css';
import { ChannelStrip } from './ChannelStrip';
import { groupPatchbay } from './devices';
import { deviceLetters, jackKey, sharedInputGroups } from './linked-inputs';
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

  return (
    <div className={styles.console}>
      <div className={styles.strips}>
        {strips.map((strip) => (
          <ChannelStrip
            key={strip.id}
            strip={strip}
            link={
              strip.input
                ? links.get(jackKey(strip.input.device ?? null, strip.input.device_channel))
                : undefined
            }
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
