import { useState } from 'react';
import { FolderOpen, HardDrive, Usb } from 'lucide-react';

import { ActionButton } from '../../design/ActionButton';
import { TextField } from '../../design/TextField';
import { loadDestinations, saveSettings, useSettings } from '../../state/settings';
import { EMPTY_STATE_COPY, emptyState, groupDrives } from './drives-logic';
import { FormatDiskModal } from './FormatDiskModal';
import { destinationStatus, owningPath } from './settings-logic';
import styles from './DestinationSection.module.css';

const TILE_ICONS = { internal: FolderOpen, usb: Usb, drive: HardDrive } as const;

/**
 * The destination panel body: one tile per plausible drive plus a custom
 * path, a REC PATH readout with its lamp, and manual rescan. Locked while
 * tape rolls — a mid-take destination change is refused daemon-side too.
 */
export function DestinationSection({ locked }: { locked: boolean }) {
  const settings = useSettings((s) => s.settings);
  const drives = useSettings((s) => s.destinations);
  const canFormat = useSettings((s) => s.canFormat);
  const error = useSettings((s) => s.error);
  const [formatting, setFormatting] = useState<string | null>(null);

  if (settings === null) {
    return <p className={styles.waiting}>waiting for the daemon…</p>;
  }

  const groups = groupDrives(settings.defaultDestination, drives, {
    canFormat,
    recording: locked,
  });
  const empty = emptyState(groups);
  const formattingGroup = groups.find((g) => g.key === formatting) ?? null;
  const selected = owningPath(
    settings.destination,
    groups.flatMap((g) => g.tiles.map((t) => t.path)).filter((p): p is string => p !== null),
  );
  const pathStatus = destinationStatus(settings.destination, settings.configuredDestination, drives);

  const choose = (path: string) => {
    if (path !== settings.destination) void saveSettings({ destination: path }, 'destination');
  };

  return (
    <div>
      {groups.map((group) => (
        <section key={group.key} className={styles.group}>
          <h3 className={styles.groupHead}>
            <span className={styles.groupTitle}>{group.title}</span>
            <span className={styles.groupDetail}>{group.detail}</span>
          </h3>
          {group.headline && (
            <p className={styles.groupHeadline} role="status">
              {group.headline}
            </p>
          )}
          {group.key !== 'internal' && (
            <p className={styles.groupActions}>
              <ActionButton
                label="Format…"
                ariaLabel={`Format ${group.title}`}
                onPress={() => setFormatting(group.key)}
                disabled={group.formatBlocked !== null}
              />
              {group.formatBlocked && (
                <span className={styles.blocked}>{group.formatBlocked}</span>
              )}
            </p>
          )}
          <div className={styles.tiles} role="listbox" aria-label={`Volumes on ${group.title}`}>
            {group.tiles.map((tile) => {
              const Icon = TILE_ICONS[tile.icon];
              const isSelected = tile.path !== null && tile.path === selected;
              return (
                <button
                  key={tile.path ?? `${group.key}:${tile.label}`}
                  type="button"
                  role="option"
                  className={styles.tile}
                  aria-selected={isSelected}
                  data-selected={isSelected || undefined}
                  data-state={tile.state}
                  disabled={locked || !tile.selectable}
                  onClick={() => tile.path !== null && choose(tile.path)}
                >
                  <Icon size={18} aria-hidden />
                  <span className={styles.tileLabel}>{tile.label}</span>
                  <span className={styles.tileDetail}>{tile.detail}</span>
                  {/* The reason is text, never colour alone — a dot that
                      only differs by hue says nothing to half the room. */}
                  <span className={styles.tileStatus} data-usable={tile.selectable || undefined}>
                    <span className={styles.statusDot} />
                    {tile.reason ?? 'ready'}
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      ))}

      {empty && (
        <p className={styles.hint} role="status">
          {EMPTY_STATE_COPY[empty]}
        </p>
      )}

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
      {formattingGroup && (
        <FormatDiskModal group={formattingGroup} open onClose={() => setFormatting(null)} />
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
