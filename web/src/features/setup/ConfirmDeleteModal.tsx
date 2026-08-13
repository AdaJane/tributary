import { useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { Modal } from '../../design/Modal';
import { TextField } from '../../design/TextField';
import { CONFIRM_WORD } from './format-logic';
import styles from './ConfirmDeleteModal.module.css';

/**
 * The typed-word gate for anything that destroys recordings.
 *
 * MASTER.md already settled the strength for this class, about the drive
 * format: "a strip costs nothing to rebuild, someone's recordings do not."
 * A two-click SURE? is right for removing a channel strip and wrong here.
 */
export function ConfirmDeleteModal({
  open,
  title,
  subject,
  detail,
  onConfirm,
  onClose,
}: {
  open: boolean;
  title: string;
  /** What is being destroyed, printed so it is never ambiguous which one. */
  subject: string;
  /** One line on the scale of the loss. */
  detail: string;
  /** Returns `null` on success, else a sentence. */
  onConfirm: () => Promise<string | null>;
  onClose: () => void;
}) {
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = typed === CONFIRM_WORD && !busy;

  const run = () => {
    setBusy(true);
    setError(null);
    void onConfirm().then((message) => {
      setBusy(false);
      if (message === null) onClose();
      else setError(message);
    });
  };

  return (
    <Modal title={title} open={open} onClose={onClose}>
      <div className={styles.body}>
        <p className={styles.warn} role="alert">
          {subject} is deleted for good. This cannot be undone.
        </p>
        <p className={styles.detail}>{detail}</p>

        <TextField
          label={`Type ${CONFIRM_WORD} to confirm`}
          value={typed}
          onCommit={setTyped}
          placeholder={CONFIRM_WORD}
          disabled={busy}
        />
        {/* The gate is the disabled state plus these words, never colour
            alone, and never a lone red button a mis-tap can reach. */}
        <p className={styles.hint} role="status">
          {busy ? 'Deleting…' : ready ? 'Ready.' : `Type ${CONFIRM_WORD} above to enable the button.`}
        </p>

        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}

        <div className={styles.actions}>
          <ActionButton label="Cancel" onPress={onClose} disabled={busy} />
          <ActionButton label={busy ? 'Deleting…' : 'Delete'} onPress={run} disabled={!ready} />
        </div>
      </div>
    </Modal>
  );
}
