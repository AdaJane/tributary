import { useEffect, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { TapeLabel } from '../../design/TapeLabel';
import { useMixer } from '../../state/mixer';
import {
  deleteSession,
  loadSessions,
  openSession,
  renameSession,
  useSessions,
} from '../../state/sessions';
import { ConfirmDeleteModal } from './ConfirmDeleteModal';
import { NewSessionModal } from './NewSessionModal';
import {
  deleteBlocked,
  openBlocked,
  sortedSessions,
  takeCountLabel,
} from './session-logic';
import styles from './SessionSection.module.css';

/**
 * The reels of tape on this drive. Each session owns its own console AND
 * its own takes, so opening one restores both — which is the whole point
 * of the panel.
 */
export function SessionSection({ locked }: { locked: boolean }) {
  const sessions = useSessions((s) => s.sessions);
  const rootPresent = useSessions((s) => s.rootPresent);
  const loaded = useSessions((s) => s.loaded);
  const error = useSessions((s) => s.error);
  const pending = useSessions((s) => s.pending);
  const strips = useMixer((s) => s.state?.strips);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  useEffect(() => {
    void loadSessions();
  }, []);

  if (!loaded) {
    return <p className={styles.waiting}>waiting for the daemon…</p>;
  }

  const rows = sortedSessions(sessions);
  const open = rows.find((s) => s.open);
  const target = rows.find((s) => s.id === deleting) ?? null;

  return (
    <div>
      <div className={styles.head}>
        <ActionButton
          label="New session…"
          onPress={() => setCreating(true)}
          disabled={locked}
        />
        {locked && <span className={styles.blocked}>locked while recording</span>}
      </div>

      {!rootPresent && (
        <p className={styles.hint} role="status">
          the recording drive is not connected — its sessions cannot be listed
        </p>
      )}
      {rootPresent && rows.length === 0 && (
        <p className={styles.hint} role="status">
          No sessions on this drive yet — start one above.
        </p>
      )}

      <div className={styles.rows} role="listbox" aria-label="Sessions">
        {rows.map((session) => {
          const cannotOpen = openBlocked(session, locked);
          const cannotDelete = deleteBlocked(session, locked);
          return (
            <div
              key={session.id}
              className={styles.row}
              role="option"
              aria-selected={session.open}
              data-open={session.open || undefined}
            >
              {/* The tape label is the rename affordance the console
                  already uses everywhere else: double-click, Enter or F2. */}
              <TapeLabel
                id={session.id}
                name={session.name}
                onRename={(name) => void renameSession(session.id, name)}
              />
              <span className={styles.meta}>
                {takeCountLabel(session.takeCount)}
                {session.open && <span className={styles.openTag}>open</span>}
              </span>
              <div className={styles.rowActions}>
                <ActionButton
                  label="Open"
                  ariaLabel={`Open ${session.name}`}
                  onPress={() => void openSession(session.id)}
                  disabled={cannotOpen !== null || pending !== null}
                />
                <ActionButton
                  label="Delete…"
                  ariaLabel={`Delete ${session.name}`}
                  onPress={() => setDeleting(session.id)}
                  disabled={cannotDelete !== null}
                />
              </div>
              {/* A blocked control keeps its reason beside it rather than
                  disappearing — a control that vanishes teaches nothing. */}
              {(cannotOpen ?? cannotDelete) && (
                <span className={styles.blocked}>{cannotOpen ?? cannotDelete}</span>
              )}
            </div>
          );
        })}
      </div>

      {error && (
        <p className={styles.error} role="alert">
          {error}
        </p>
      )}

      {creating && (
        <NewSessionModal
          open
          onClose={() => setCreating(false)}
          stripCount={strips?.length ?? 0}
          leaving={open?.name ?? 'this session'}
        />
      )}
      {target && (
        <ConfirmDeleteModal
          open
          title={`Delete ${target.name}`}
          subject={target.name}
          detail={`${takeCountLabel(target.takeCount)} — the audio is removed from the drive.`}
          onConfirm={() => deleteSession(target.id)}
          onClose={() => setDeleting(null)}
        />
      )}
    </div>
  );
}
