import { useEffect } from 'react';

import { API_BASE_URL } from '../../api/client';
import { useMixer } from '../../state/mixer';
import { usePeaksStore } from '../../state/peaks';
import { useTakes } from '../../state/takes';
import { useTransport } from '../../state/transport';
import { wsClient } from '../../ws/client-instance';
import { parsePeaks } from './peaks-parse';
import { peaksErrorMessage } from './take-errors';

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

  // Metadata comes from the take store, which the browser keeps current
  // anyway. This used to GET the whole take list on every take change and
  // throw away every row but one.
  const info = useTakes((s) => s.takes.find((t) => t.take === take) ?? null);

  useEffect(() => {
    if (take === null) {
      // The shelf emptied — stop painting a take that is gone.
      usePeaksStore.getState().clear();
      return;
    }
    if (recording || info === null) return;
    let cancelled = false;
    const store = usePeaksStore.getState();
    store.beginLoad(take);
    // Every failure below says something. This whole path used to end in
    // `.catch(() => undefined)` with a non-ok response resolving to null,
    // so a broken take rendered as an empty room and nothing else.
    void fetch(`${API_BASE_URL}/api/v1/takes/${take}/peaks`)
      .then(async (res) => {
        if (cancelled) return;
        if (!res.ok) {
          usePeaksStore.getState().failLoad(peaksErrorMessage(res.status));
          return;
        }
        const parsed = parsePeaks(await res.arrayBuffer());
        if (cancelled) return;
        if (!parsed) {
          usePeaksStore.getState().failLoad(peaksErrorMessage('malformed'));
          return;
        }
        usePeaksStore.getState().setPeaks(
          take,
          parsed,
          info.tracks.map((t) => ({
            file: t.file,
            channels: t.channels,
            stripId: t.stripId,
            damaged: t.damaged,
          })),
        );
      })
      .catch(() => {
        if (!cancelled) usePeaksStore.getState().failLoad(peaksErrorMessage('network'));
      });
    return () => {
      cancelled = true;
    };
  }, [take, recording, info]);
}
