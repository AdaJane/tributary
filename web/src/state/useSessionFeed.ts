/**
 * Keeps the open session current for the whole app.
 *
 * Mounted once in the shell rather than per-view: the console's tape
 * label needs the session's name from boot, and Setup is lazy — it may
 * never mount at all.
 */
import { useEffect } from 'react';

import { loadSessions, useSessions } from './sessions';
import { wsClient } from '../ws/client-instance';

export function useSessionFeed(): void {
  useEffect(() => {
    const release = wsClient.subscribe({ kind: 'sessions' });
    const detach = wsClient.onMessage((message) => {
      if (message.type !== 'sessions_changed') return;
      // The push carries only which session is open — building the whole
      // list would mean a directory walk on the control task. Patch the
      // open flag in place and refetch only if we have never listed.
      const { sessions, loaded } = useSessions.getState();
      if (!loaded) {
        void loadSessions();
        return;
      }
      useSessions.setState({
        sessions: sessions.map((s) => ({
          ...s,
          open: s.id === message.open_id,
          name: s.id === message.open_id ? message.open_name : s.name,
        })),
      });
      // A create adds a row we have never seen; a delete removes one.
      if (!sessions.some((s) => s.id === message.open_id)) void loadSessions();
    });
    void loadSessions();
    return () => {
      detach();
      release();
    };
  }, []);
}
