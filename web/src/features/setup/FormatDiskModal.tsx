import { useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { Modal } from '../../design/Modal';
import { SegmentedControl } from '../../design/SegmentedControl';
import { TextField } from '../../design/TextField';
import { formatDrive } from '../../state/settings';
import type { DriveGroup } from './drives-logic';
import type { Filesystem } from './format-logic';
import {
  CONFIRM_WORD,
  DEFAULT_LABEL,
  FILESYSTEMS,
  FILESYSTEM_COPY,
  confirmReady,
  defaultFilesystem,
  formatPromise,
  validateLabel,
} from './format-logic';
import styles from './FormatDiskModal.module.css';

/**
 * The wipe dialog. Irreversible, and reachable by anyone on the
 * appliance's open Wi-Fi, so the gate is a typed word rather than the
 * two-click SURE? used for removing a strip — and the drive's current
 * contents are printed in full, because "which stick is /dev/sda" is not
 * a question anyone should answer from memory.
 */
export function FormatDiskModal({
  group,
  open,
  onClose,
}: {
  group: DriveGroup;
  open: boolean;
  onClose: () => void;
}) {
  const [label, setLabel] = useState(DEFAULT_LABEL);
  // Defaulted from where the drive lives, not locked to it: a stick you
  // never intend to unplug is a perfectly good ext4 drive.
  const [filesystem, setFilesystem] = useState<Filesystem>(() =>
    defaultFilesystem(group.removable),
  );
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const labelError = validateLabel(label);
  const ready = confirmReady(typed, label) && !busy;

  const run = () => {
    setBusy(true);
    setError(null);
    void formatDrive(group.key, label, filesystem).then((message) => {
      setBusy(false);
      if (message === null) {
        // Success needs no announcement here: the drive remounts and the
        // pushed destination list redraws the panel behind this dialog.
        onClose();
      } else {
        setError(message);
      }
    });
  };

  return (
    <Modal title={`Erase ${group.title}`} open={open} onClose={onClose}>
      <div className={styles.body}>
        <p className={styles.warn} role="alert">
          Everything on this drive is erased. This cannot be undone.
        </p>

        <section className={styles.panel}>
          <h4 className={styles.panelHead}>Now</h4>
          <p className={styles.device}>
            {group.key} · {group.detail}
          </p>
          <ul className={styles.rows}>
            {group.tiles.map((t) => (
              <li key={t.label} className={styles.row}>
                <span>{t.label}</span>
                <span className={styles.rowDetail}>{t.detail}</span>
              </li>
            ))}
          </ul>
        </section>

        <SegmentedControl
          label="Filesystem"
          options={FILESYSTEMS.map((fs) => ({
            value: fs,
            label: FILESYSTEM_COPY[fs].label,
          }))}
          value={filesystem}
          onChange={setFilesystem}
          disabled={busy}
        />
        {/* The consequence in words, because "exFAT" and "ext4" tell you
            nothing about which one you want. */}
        <p className={styles.hint} role="status">
          {FILESYSTEM_COPY[filesystem].detail}
        </p>

        <section className={styles.panel}>
          <h4 className={styles.panelHead}>After</h4>
          <p className={styles.promise}>
            {formatPromise(group.totalBytes, label, filesystem)}
          </p>
        </section>

        <TextField
          label="Drive name"
          value={label}
          onCommit={(v) => setLabel(v.toUpperCase())}
          placeholder={DEFAULT_LABEL}
          disabled={busy}
        />
        {labelError && (
          <p className={styles.hint} role="status">
            {labelError}
          </p>
        )}

        <TextField
          label={`Type ${CONFIRM_WORD} to confirm`}
          value={typed}
          onCommit={setTyped}
          placeholder={CONFIRM_WORD}
          disabled={busy}
        />
        {/* The gate is the disabled state plus these words — never colour
            alone, and never a lone red button that a mis-tap can reach. */}
        <p className={styles.hint} role="status">
          {busy
            ? 'Erasing — this takes a few seconds.'
            : ready
              ? 'Ready.'
              : `Type ${CONFIRM_WORD} above to enable the button.`}
        </p>

        {error && (
          <p className={styles.error} role="alert">
            {error}
          </p>
        )}

        <div className={styles.actions}>
          <ActionButton label="Cancel" onPress={onClose} disabled={busy} />
          <ActionButton
            label={busy ? 'Erasing…' : 'Erase and format'}
            onPress={run}
            disabled={!ready}
          />
        </div>
      </div>
    </Modal>
  );
}
