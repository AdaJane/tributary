import { useEffect } from 'react';

import { FrameBatcher } from '../../state/batcher';
import { useConnection } from '../../state/connection';
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
      // Two publishes can land inside one flush window (jitter, or a
      // socket catching up after a lag). The daemon coalesces by max and
      // OR before it publishes; taking the last here instead would throw
      // away the louder of the two — and the clip flag with it.
      (prev, next) => ({
        peakDb: Math.max(prev.peakDb, next.peakDb),
        clip: prev.clip || next.clip,
      }),
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
    // Nothing feeding the meters means silence, not the last frame that
    // made it through. Without this every LED freezes mid-reading when the
    // socket drops — the console's most alarming lie, because a held meter
    // looks exactly like a live one.
    const unwatch = useConnection.subscribe((state, prev) => {
      if (state.status !== 'online' && prev.status === 'online') {
        useMeters.getState().clear();
      }
    });
    return () => {
      detach();
      release();
      batcher.stop();
      unwatch();
      useMeters.getState().clear();
    };
  }, []);
}
