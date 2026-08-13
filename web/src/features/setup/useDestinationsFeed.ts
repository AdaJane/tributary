/**
 * Live drive list. The daemon pushes on every mount-table change, so a
 * drive plugged in while Setup is open appears on its own — no Rescan, and
 * nothing polls on either side.
 *
 * The subscribe carries a snapshot, so this is also correct on a cold open
 * and after any reconnect; the REST load stays as the belt for a socket
 * that is still connecting.
 */
import { useEffect } from 'react';

import { applyDestinations, loadDestinations } from '../../state/settings';
import { wsClient } from '../../ws/client-instance';

export function useDestinationsFeed(): void {
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'destinations' });
    const detach = wsClient.onMessage((message) => {
      if (message.type === 'destinations') applyDestinations(message.drives);
    });
    void loadDestinations();
    return () => {
      detach();
      release();
    };
  }, []);
}
