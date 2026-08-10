import { FolderOpen, HardDrive, Usb } from 'lucide-react';

import { ActionButton } from '../../design/ActionButton';
import { TextField } from '../../design/TextField';
import { loadDestinations, saveSettings, useSettings } from '../../state/settings';
import type { Destination, RecordingSettings } from '../../state/settings';
import { destinationStatus, formatBytes, owningPath } from './settings-logic';
import styles from './DestinationSection.module.css';

/** Where a drive tile points recording: a tidy subdir, not the drive root. */
const DRIVE_SUBDIR = 'tributary';

interface Tile {
  path: string;
  label: string;
  detail: string;
  icon: 'internal' | 'usb' | 'drive';
  status: 'ready' | 'read-only';
}

function driveTiles(settings: RecordingSettings, drives: readonly Destination[]): Tile[] {
  const internal: Tile = {
    path: settings.defaultDestination,
    label: 'Internal',
    detail: 'default',
    icon: 'internal',
    status: 'ready',
  };
  const mounted = drives
    .filter((d) => d.mountPath !== '/')
    .map<Tile>((d) => ({
      path: `${d.mountPath}/${DRIVE_SUBDIR}`,
      label: d.label,
      detail: `${formatBytes(d.freeBytes)} free`,
      icon: d.removable ? 'usb' : 'drive',
      status: d.writable ? 'ready' : 'read-only',
    }));
  return [internal, ...mounted];
}

const TILE_ICONS = { internal: FolderOpen, usb: Usb, drive: HardDrive } as const;

/**
 * The destination panel body: one tile per plausible drive plus a custom
 * path, a REC PATH readout with its lamp, and manual rescan. Locked while
 * tape rolls — a mid-take destination change is refused daemon-side too.
 */
export function DestinationSection({ locked }: { locked: boolean }) {
  const settings = useSettings((s) => s.settings);
  const drives = useSettings((s) => s.destinations);
  const error = useSettings((s) => s.error);

  if (settings === null) {
    return <p className={styles.waiting}>waiting for the daemon…</p>;
  }

  const tiles = driveTiles(settings, drives);
  const selected = owningPath(
    settings.destination,
    tiles.map((t) => t.path),
  );
  const pathStatus = destinationStatus(settings.destination, settings.configuredDestination, drives);

  const choose = (path: string) => {
    if (path !== settings.destination) void saveSettings({ destination: path }, 'destination');
  };

  return (
    <div>
      <div className={styles.tiles} role="listbox" aria-label="Destination drive">
        {tiles.map((tile) => {
          const Icon = TILE_ICONS[tile.icon];
          return (
            <button
              key={tile.path}
              type="button"
              role="option"
              className={styles.tile}
              aria-selected={tile.path === selected}
              data-selected={tile.path === selected || undefined}
              data-status={tile.status}
              disabled={locked || tile.status === 'read-only'}
              onClick={() => choose(tile.path)}
            >
              <Icon size={18} aria-hidden />
              <span className={styles.tileLabel}>{tile.label}</span>
              <span className={styles.tileDetail}>{tile.detail}</span>
              <span className={styles.tileStatus} data-status={tile.status}>
                <span className={styles.statusDot} />
                {tile.status}
              </span>
            </button>
          );
        })}
      </div>

      <div className={styles.custom}>
        <span className={styles.customLabel}>Custom</span>
        <TextField
          label="Custom destination path"
          value={settings.destination}
          onCommit={(path) => choose(path)}
          placeholder="/absolute/path"
          disabled={locked}
        />
        <ActionButton label="Rescan" onPress={() => void loadDestinations()} />
      </div>

      <p className={styles.recPath} data-status={pathStatus}>
        <span className={styles.recPathLabel}>Rec path</span>
        <span className={styles.recPathValue}>{settings.destination}</span>
        <span className={styles.tileStatus} data-status={pathStatus}>
          <span className={styles.statusDot} />
          {pathStatus}
        </span>
      </p>
      {pathStatus === 'missing' && (
        <p className={styles.hint} role="status">
          the configured drive was missing at start — recording to the path above instead
        </p>
      )}
      {locked && (
        <p className={styles.lockHint} role="status">
          destination locked while recording
        </p>
      )}
      {error?.field === 'destination' && (
        <p className={styles.error} role="status">
          <span className={styles.statusDot} />
          {error.message}
        </p>
      )}
    </div>
  );
}
