import { useMemo, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { Modal } from '../../design/Modal';
import { removeTake, selectTake, useTakes } from '../../state/takes';
import { useTransport } from '../../state/transport';
import {
  sortedTakes,
  takeDuration,
  takeLabel,
  takePlayability,
  takeTime,
  trackCountLabel,
} from './take-list';
import styles from './TakeBrowserModal.module.css';

/**
 * Every take in the open session, and a way onto any of them.
 *
 * A modal rather than a panel: the Tracks view is mostly waveform and
 * vertical space is scarce on a phone, and this is the same
 * pick-one-from-a-list problem the input patchbay already solves this way.
 */
export function TakeBrowserModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const takes = useTakes((s) => s.takes);
  const error = useTakes((s) => s.error);
  const pending = useTakes((s) => s.pending);
  const selected = useTransport((s) => s.take);
  const engineRate = useTransport((s) => s.engineSampleRate);
  const recording = useTransport((s) => s.phase === 'recording');
  const [confirming, setConfirming] = useState<number | null>(null);

  // Times are locale-formatted once, not per row per render.
  const clock = useMemo(
    () => new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }),
    [],
  );
  const rows = sortedTakes(takes);
  const target = rows.find((t) => t.take === selected) ?? null;

  const choose = (take: number) => {
    setConfirming(null);
    void selectTake(take).then((message) => {
      if (message === null) onClose();
    });
  };

  return (
    <Modal title="Takes" open={open} onClose={onClose}>
      <div className={styles.body}>
        {recording && (
          <p className={styles.hint} role="status">
            tape is rolling — takes cannot be switched until it stops
          </p>
        )}

        {rows.length === 0 ? (
          <p className={styles.hint} role="status">
            No takes in this session yet — arm a channel and hit REC.
          </p>
        ) : (
          <div className={styles.rows} role="listbox" aria-label="Takes">
            {rows.map((take) => {
              const { playable, reason } = takePlayability(take.sampleRate, engineRate);
              const isSelected = take.take === selected;
              return (
                <button
                  key={take.take}
                  type="button"
                  role="option"
                  className={styles.row}
                  aria-selected={isSelected}
                  data-selected={isSelected || undefined}
                  disabled={recording || pending !== null}
                  onClick={() => choose(take.take)}
                >
                  <span className={styles.take}>{takeLabel(take.take)}</span>
                  <span className={styles.time}>{takeTime(take.startedAtUnix, clock)}</span>
                  <span className={styles.duration}>{takeDuration(take.durationSecs)}</span>
                  <span className={styles.tracks}>{trackCountLabel(take.tracks.length)}</span>
                  {take.damaged && <span className={styles.damaged}>DAMAGED</span>}
                  {/* A mismatched take is still selectable and viewable —
                      only PLAY is blocked, and it says so here rather than
                      surfacing as a refusal after the fact. */}
                  {!playable && <span className={styles.unplayable}>{reason}</span>}
                </button>
              );
            })}
          </div>
        )}

        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}

        {/* Delete acts on the SELECTED take only: no per-row ✕ to
            fat-finger on a phone, and a two-step rather than a nested
            dialog, which is fiddly on iOS Safari. */}
        {target && (
          <div className={styles.footer}>
            {confirming === target.take ? (
              <>
                <span className={styles.confirm}>
                  Delete {takeLabel(target.take)}? The audio is removed from the drive. This cannot
                  be undone.
                </span>
                <ActionButton label="Cancel" onPress={() => setConfirming(null)} />
                <ActionButton
                  label="Delete"
                  onPress={() => {
                    setConfirming(null);
                    void removeTake(target.take);
                  }}
                />
              </>
            ) : (
              <ActionButton
                label={`Delete ${takeLabel(target.take)}…`}
                onPress={() => setConfirming(target.take)}
                disabled={recording || pending !== null}
              />
            )}
          </div>
        )}
      </div>
    </Modal>
  );
}
