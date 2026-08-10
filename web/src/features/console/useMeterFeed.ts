import { useEffect } from 'react';

import { FrameBatcher } from '../../state/batcher';
import { useMeters } from '../../state/meters';
import { wsClient } from '../../ws/client-instance';
import { meterKeyString } from '../../ws/messages';

/** LED-live cadence: the daemon publishes at 20 Hz; flushing at 50 ms keeps
 * one render per publish without ever queueing a backlog. */
const METER_FLUSH_MS = 50;

export function useMeterFeed(): void {
  useEffect(() => {
    const batcher = new FrameBatcher<{ peakDb: number; clip: boolean }>(
      METER_FLUSH_MS,
      (batch) => useMeters.getState().applyBatch(batch, Date.now()),
    );
    const release = wsClient.subscribe({ kind: 'meters' });
    const detach = wsClient.onMessage((message) => {
      if (message.type !== 'meters') return;
      for (const meter of message.meters) {
        batcher.push(meterKeyString(meter.key), {
          peakDb: meter.peak_db,
          clip: meter.clip,
        });
      }
    });
    return () => {
      detach();
      release();
      batcher.stop();
    };
  }, []);
}
