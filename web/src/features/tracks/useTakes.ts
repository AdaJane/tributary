/**
 * Live take list for the open session. Seeded by REST, then kept current
 * by the `takes_changed` push — which rides the transport channel every
 * console already subscribes to for the playhead, so this costs no extra
 * subscription.
 */
import { useEffect } from 'react';

import { applyTakes, loadTakes } from '../../state/takes';
import { wsClient } from '../../ws/client-instance';

export function useTakesFeed(): void {
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'transport' });
    const detach = wsClient.onMessage((message) => {
      if (message.type === 'takes_changed') applyTakes(message.takes);
    });
    void loadTakes();
    return () => {
      detach();
      release();
    };
  }, []);
}
