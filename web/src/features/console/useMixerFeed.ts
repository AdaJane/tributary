import { useEffect } from 'react';

import { useMixer } from '../../state/mixer';
import { wsClient } from '../../ws/client-instance';
import { acceptEcho } from '../../ws/send';

/** Live mixer document: snapshot on subscribe, deltas filtered through the
 * echo rule. Mounted once by the console view. */
export function useMixerFeed(): void {
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'mixer' });
    const detach = wsClient.onMessage((message) => {
      if (message.type === 'mixer_snapshot') {
        useMixer.getState().applySnapshot(message.state);
      } else if (
        message.type === 'state_changed' &&
        acceptEcho(message.delta, message.ack)
      ) {
        useMixer.getState().applyDelta(message.delta);
      }
    });
    return () => {
      detach();
      release();
    };
  }, []);
}
