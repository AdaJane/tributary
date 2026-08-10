import { useCallback, useEffect, useRef, useState } from 'react';

import { MONITOR_WS_URL } from '../../api/client';
import { useTransport } from '../../state/transport';
import { MonitorPlayer } from './player';

/** Runs the browser monitor whenever the daemon's monitor targets the
 * stream. `needsTap` surfaces the autoplay gate; `enable` is the tap. */
export function useMonitorStream(): { needsTap: boolean; enable: () => void } {
  const streaming = useTransport((s) => s.monitor === 'stream');
  const [needsTap, setNeedsTap] = useState(false);
  const playerRef = useRef<MonitorPlayer | null>(null);

  useEffect(() => {
    if (!streaming) {
      setNeedsTap(false);
      return;
    }
    const player = new MonitorPlayer();
    playerRef.current = player;
    void player
      .start(MONITOR_WS_URL)
      .then(() => setNeedsTap(player.suspended))
      .catch(() => setNeedsTap(false));
    return () => {
      player.stop();
      playerRef.current = null;
    };
  }, [streaming]);

  const enable = useCallback(() => {
    void playerRef.current
      ?.resume()
      .then(() => setNeedsTap(playerRef.current?.suspended ?? false));
  }, []);

  return { needsTap, enable };
}
