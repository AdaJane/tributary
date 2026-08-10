import { useEffect } from 'react';

import { $api, API_BASE_URL } from '../../api/client';
import { useMixer } from '../../state/mixer';
import { usePeaksStore } from '../../state/peaks';
import { useTransport } from '../../state/transport';
import { wsClient } from '../../ws/client-instance';
import { parsePeaks } from './peaks-parse';

/** Keep the take document current: finished takes load wholesale; while
 * recording, lanes for the armed strips grow from the Waveform channel. */
export function usePeaks(): void {
  const take = useTransport((s) => s.take);
  const recording = useTransport((s) => s.phase === 'recording');

  // The moment tape rolls, start a live document — lane order mirrors the
  // daemon's sink order: armed strips in strip order, then the master.
  useEffect(() => {
    if (!recording) return;
    const transport = useTransport.getState();
    const mixer = useMixer.getState().state;
    const meta = [
      ...mixer.strips
        .filter((strip) => strip.record_arm)
        .map((strip) => ({ file: '', channels: 1, stripId: strip.id, damaged: false })),
      ...(mixer.master.record_arm
        ? [{ file: 'master.wav', channels: 2, stripId: null, damaged: false }]
        : []),
    ];
    usePeaksStore.getState().startLive(transport.take ?? 0, meta);
  }, [recording]);

  // Live bins ride their own channel; appendBins drops stale takes.
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'waveform' });
    const detach = wsClient.onMessage((message) => {
      if (message.type === 'waveform_bins') {
        usePeaksStore
          .getState()
          .appendBins(message.take, message.track, message.start_bin, message.bins);
      }
    });
    return () => {
      detach();
      release();
    };
  }, []);

  useEffect(() => {
    if (take === null || recording) return;
    let cancelled = false;
    void Promise.all([
      fetch(`${API_BASE_URL}/api/v1/takes/${take}/peaks`).then((res) =>
        res.ok ? res.arrayBuffer() : null,
      ),
      $api.GET('/api/v1/takes').then(({ data }) => data?.find((t) => t.take === take) ?? null),
    ])
      .then(([buffer, info]) => {
        if (cancelled || !buffer || !info) return;
        const parsed = parsePeaks(buffer);
        if (!parsed) return;
        const meta = info.tracks.map((t) => ({
          file: t.file,
          channels: t.channels,
          stripId: t.strip_id ?? null,
          damaged: t.dropped_samples > 0,
        }));
        usePeaksStore.getState().setPeaks(take, parsed, meta);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [take, recording]);
}
