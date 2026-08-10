import { Component } from 'react';

import styles from './ErrorBoundary.module.css';

interface Props {
  children: React.ReactNode;
}

interface State {
  error: Error | null;
}

/** A crashed panel must never black out the console — show the fault and
 * offer a reload, like a channel strip you can re-seat. */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  render() {
    if (this.state.error === null) return this.props.children;
    return (
      <div className={styles.wrap} role="alert">
        <p className={styles.tape}>Something popped</p>
        <pre className={styles.detail}>{this.state.error.message}</pre>
        <button
          type="button"
          className={styles.reload}
          onClick={() => window.location.reload()}
        >
          Reload the console
        </button>
      </div>
    );
  }
}
