import { useRef, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { InlineError } from '../../design/Panel';
import { API_BASE_URL } from '../../api/client';
import { useInstruments } from '../../state/instruments';
import type { InstrumentsDocument } from '../../state/instruments';
import {
  deleteBlocked,
  emptyState,
  formatBytes,
  groupSoundfonts,
  progressLine,
  uploadErrorMessage,
  validateUpload,
} from './soundfont-logic';
import styles from './InstrumentsView.module.css';

/**
 * The library, and the one control that adds to it.
 *
 * The upload uses **XMLHttpRequest**, not `fetch`, and that is deliberate:
 * `xhr.upload.onprogress` is the only progress source that works on a plain
 * HTTP origin, and this console is permanently on one (the appliance serves
 * the LAN over http://tributary.local). A streamed `fetch` body needs a
 * secure context, so "modernising" this would silently lose both the
 * progress bar and cancellation.
 */
export function SoundfontSection({
  doc,
  recording,
}: {
  doc: InstrumentsDocument;
  recording: boolean;
}) {
  const load = useInstruments((s) => s.load);
  const removeSoundfont = useInstruments((s) => s.deleteSoundfont);
  const picker = useRef<HTMLInputElement>(null);
  const xhr = useRef<XMLHttpRequest | null>(null);
  const [progress, setProgress] = useState<{ sent: number; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const groups = groupSoundfonts(doc.soundfonts);
  const empty = emptyState(doc.soundfonts, groups.length > 1);

  const upload = (file: File) => {
    const refusal = validateUpload(file.name, file.size, doc.maxUploadBytes);
    if (refusal) {
      setError(refusal);
      return;
    }
    setError(null);
    setNote(null);
    setProgress({ sent: 0, total: file.size });

    const request = new XMLHttpRequest();
    xhr.current = request;
    request.open('POST', `${API_BASE_URL}/api/v1/soundfonts/${encodeURIComponent(file.name)}`);
    request.setRequestHeader('Content-Type', 'application/octet-stream');
    request.upload.onprogress = (e) =>
      setProgress({ sent: e.loaded, total: e.total || file.size });
    request.onload = () => {
      xhr.current = null;
      setProgress(null);
      if (request.status >= 200 && request.status < 300) {
        setNote(`Added ${file.name}.`);
        void load();
      } else {
        setError(uploadErrorMessage(request.status, detailOf(request.responseText)));
      }
    };
    request.onerror = () => {
      xhr.current = null;
      setProgress(null);
      setError(uploadErrorMessage(0));
    };
    request.onabort = () => {
      xhr.current = null;
      setProgress(null);
      setNote('Upload cancelled — nothing was added.');
    };
    request.send(file);
  };

  return (
    <>
      {empty && <p className={styles.empty}>{empty}</p>}

      {groups.map((group) =>
        group.files.length === 0 && !group.removable ? null : (
          <div key={group.title} className={styles.group}>
            <p className={styles.groupTitle}>{group.title}</p>
            <ul className={styles.files}>
              {group.files.map((file) => {
                const usedBy = doc.instruments
                  .filter((i) => i.soundfont === file.id)
                  .map((i) => i.name);
                const blocked = deleteBlocked(file, usedBy, recording);
                return (
                  <li key={file.path} className={styles.file}>
                    <span className={styles.fileName}>{file.id}</span>
                    <span className={styles.hint}>{formatBytes(file.bytes)}</span>
                    <ActionButton
                      label="Remove"
                      ariaLabel={`Remove ${file.id} from the library`}
                      onPress={() => void removeSoundfont(file.id)}
                      disabled={blocked !== null}
                    />
                    {blocked && <span className={styles.blocked}>{blocked}</span>}
                  </li>
                );
              })}
            </ul>
          </div>
        ),
      )}

      <div className={styles.uploadRow}>
        <input
          ref={picker}
          type="file"
          accept=".sf2"
          className={styles.hiddenInput}
          onChange={(e) => {
            const file = e.target.files?.[0];
            if (file) upload(file);
            e.target.value = '';
          }}
        />
        <ActionButton
          label="Upload soundfont…"
          onPress={() => picker.current?.click()}
          disabled={recording || progress !== null}
        />
        {recording && <span className={styles.blocked}>stop recording first</span>}
        {progress && (
          <>
            <span className={styles.hint} role="status">
              {progressLine(progress.sent, progress.total)}
            </span>
            <ActionButton
              label="Cancel upload"
              onPress={() => xhr.current?.abort()}
            />
          </>
        )}
      </div>

      <p className={styles.path}>Uploads land in {doc.soundfontDir}</p>
      {note && (
        <p className={styles.hint} role="status">
          {note}
        </p>
      )}
      {error && <InlineError message={error} />}
    </>
  );
}

function detailOf(body: string): string | undefined {
  try {
    const parsed = JSON.parse(body) as { detail?: unknown };
    return typeof parsed.detail === 'string' ? parsed.detail : undefined;
  } catch {
    return undefined;
  }
}
