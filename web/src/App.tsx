import { useEffect } from 'react';

import { AppShell } from './app/AppShell';
import { ErrorBoundary } from './app/ErrorBoundary';
import { wsClient } from './ws/client-instance';

export function App() {
  // One socket per tab, alive for the app's lifetime.
  useEffect(() => {
    wsClient.start();
    return () => wsClient.stop();
  }, []);
  return (
    <ErrorBoundary>
      <AppShell />
    </ErrorBoundary>
  );
}
