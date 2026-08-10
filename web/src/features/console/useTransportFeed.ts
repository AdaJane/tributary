import { useEffect } from 'react';

import { $api } from '../../api/client';
import { useTransport } from '../../state/transport';
import { wsClient } from '../../ws/client-instance';

/** Transport pushes carry state changes; the REST read seeds the initial
 * value (the channel has no snapshot-on-subscribe semantics). */
export function useTransportFeed(): void {
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'transport' });
    const detach = wsClient.onMessage((message) => {
      if (message.type === 'transport') {
        useTransport.getState().apply(message.state);
      } else if (message.type === 'playback_position') {
        useTransport.getState().applyPosition(message.frames);
      }
    });
    void $api.GET('/api/v1/transport').then(({ data }) => {
      if (data) useTransport.getState().apply(data);
    });
    return () => {
      detach();
      release();
    };
  }, []);
}
