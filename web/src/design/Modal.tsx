import { X } from 'lucide-react';
import { useEffect, useRef } from 'react';

import styles from './Modal.module.css';

export interface ModalProps {
  title: string;
  open: boolean;
  onClose: () => void;
  children: React.ReactNode;
}

/** Native `<dialog>` under the hood: focus trap, Esc-to-close, and focus
 * return to the opener all come from the platform. */
export function Modal({ title, open, onClose, children }: ModalProps) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className={styles.dialog}
      aria-label={title}
      onClose={onClose}
      onCancel={onClose}
    >
      {open && (
        <>
          <header className={styles.header}>
            <h2 className={styles.title}>{title}</h2>
            <button
              type="button"
              className={styles.close}
              aria-label="Close"
              onClick={onClose}
            >
              <X size={14} aria-hidden />
            </button>
          </header>
          <div className={styles.body}>{children}</div>
        </>
      )}
    </dialog>
  );
}
