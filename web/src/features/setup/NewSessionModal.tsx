import { useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { Modal } from '../../design/Modal';
import { SegmentedControl } from '../../design/SegmentedControl';
import type { SegmentedOption } from '../../design/SegmentedControl';
import { TextField } from '../../design/TextField';
import { createSession } from '../../state/sessions';
import { SEED_LABELS, seedConsequence, seedNeedsConfirm, validateSessionName } from './session-logic';
import type { SessionSeed } from './session-logic';
import styles from './NewSessionModal.module.css';

const SEED_OPTIONS: readonly SegmentedOption<SessionSeed>[] = [
  { value: 'template', label: SEED_LABELS.template },
  { value: 'mapping', label: SEED_LABELS.mapping },
  { value: 'console', label: SEED_LABELS.console },
];

/**
 * Tear off fresh tape. The seed choice is the substance here: on an
 * appliance you are usually starting the next song on a rig you already
 * wired, so `mapping` is the default — the option that is wrong least
 * often.
 */
export function NewSessionModal({
  open,
  onClose,
  stripCount,
  leaving,
}: {
  open: boolean;
  onClose: () => void;
  stripCount: number;
  leaving: string;
}) {
  const [name, setName] = useState('');
  const [seed, setSeed] = useState<SessionSeed>('mapping');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const nameError = name.length > 0 ? validateSessionName(name) : null;
  const ready = validateSessionName(name) === null && !busy;

  const run = () => {
    setBusy(true);
    setError(null);
    void createSession(name.trim(), seed).then((message) => {
      setBusy(false);
      if (message === null) onClose();
      else setError(message);
    });
  };

  return (
    <Modal title="New session" open={open} onClose={onClose}>
      <div className={styles.body}>
        <TextField
          label="Session name"
          value={name}
          onCommit={setName}
          placeholder="Friday Night"
          disabled={busy}
        />
        {nameError && (
          <p className={styles.hint} role="status">
            {nameError}
          </p>
        )}

        <div className={styles.seed}>
          <SegmentedControl
            label="Start from"
            options={SEED_OPTIONS}
            value={seed}
            onChange={setSeed}
          />
          {/* The label says what you get; this says what you lose, and it
              updates with the choice so the cost is never a surprise. */}
          <p className={styles.consequence} role="status">
            {seedConsequence(seed, stripCount, leaving)}
          </p>
        </div>

        {/* Only a seed that changes the desk is worth confirming — a
            warning that fires when nothing will happen gets tapped
            through without reading. */}
        {seedNeedsConfirm(seed) && (
          <p className={styles.warn} role="status">
            This replaces the console you are looking at.
          </p>
        )}

        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}

        <div className={styles.actions}>
          <ActionButton label="Cancel" onPress={onClose} disabled={busy} />
          <ActionButton
            label={busy ? 'Creating…' : 'Create session'}
            onPress={run}
            disabled={!ready}
          />
        </div>
      </div>
    </Modal>
  );
}
