import { useEffect, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { Modal } from '../../design/Modal';
import { refreshDevices, setCardProfile, useDevices } from '../../state/devices';
import type { StripState } from '../../ws/messages';
import styles from './InputPickerModal.module.css';
import { SourceIcon } from './SourceIcon';
import { PRO_AUDIO_PROFILE, groupPatchbay, sourceKind } from './devices';
import { jackKey, jackLabel, deviceLetters, stripeColor } from './linked-inputs';

export interface PatchTarget {
  device: string | null;
  channel: number;
}

export interface InputPickerModalProps {
  /** The strip being patched. */
  strip: StripState;
  /** The whole console — assignments elsewhere are shown, not hidden. */
  strips: readonly StripState[];
  open: boolean;
  onClose: () => void;
  /** `null` disconnects. The caller performs the mutation and closes. */
  onPatch: (target: PatchTarget | null) => void;
}

const STATUS_LABEL = {
  open: 'live',
  available: 'ready',
  failed: 'failed',
  absent: 'missing',
} as const;

/** The patchbay: one lettered stage box per input device, every jack a
 * tile — including jacks feeding other strips (sharing is legal; that's
 * what the tape stripe marks). Opening refreshes: the daemon re-enumerates
 * and retries anything that failed, so plugging in and reopening is enough. */
export function InputPickerModal({
  strip,
  strips,
  open,
  onClose,
  onPatch,
}: InputPickerModalProps) {
  const currentDevice = strip.input ? (strip.input.device ?? null) : undefined;
  const currentChannel = strip.input?.device_channel;
  const devices = useDevices((s) => s.devices);
  const pendingCard = useDevices((s) => s.pendingCard);
  const [profileError, setProfileError] = useState<{ card: string; message: string } | null>(
    null,
  );

  useEffect(() => {
    if (open) void refreshDevices();
  }, [open]);

  const changeProfile = async (card: string, profile: string) => {
    setProfileError(null);
    const message = await setCardProfile(card, profile);
    if (message) setProfileError({ card, message });
  };

  const sections = groupPatchbay(devices, strips);
  const anyProfiles = sections.some((s) => s.profiles.length > 0);
  const letters = deviceLetters(
    sections.filter((s) => s.device !== null).map((s) => s.device as string),
  );

  return (
    <Modal title={`Patch input — ${strip.name}`} open={open} onClose={onClose}>
      {sections.map((section) => (
        <section key={section.device ?? ''} className={styles.deviceSection}>
          <p className={styles.device}>
            <span className={styles.letter}>{section.letter}</span>
            <span className={styles.deviceIcon}>
              <SourceIcon kind={sourceKind(section.device, section.title)} />
            </span>
            <span className={styles.deviceName}>
              {section.title}
              {section.sublabel && (
                <span
                  className={styles.deviceHint}
                  aria-label={`follows ${section.sublabel}`}
                >
                  {' ↳ '}
                  {section.sublabel}
                </span>
              )}
              {section.reconciledFrom && (
                <span className={styles.deviceHint}> — was: {section.reconciledFrom}</span>
              )}
            </span>
            <span className={styles.status} data-status={section.status}>
              <span className={styles.statusDot} aria-hidden />
              {STATUS_LABEL[section.status]}
            </span>
          </p>
          {section.status === 'failed' && section.error && (
            <p className={styles.deviceError} role="alert">
              {section.error}
            </p>
          )}
          {section.card && section.profiles.length > 0 && (
            <p className={styles.profile}>
              <label className={styles.profileLabel} htmlFor={`mode-${section.letter}`}>
                Mode
              </label>
              <select
                id={`mode-${section.letter}`}
                className={styles.profileSelect}
                value={section.profile ?? ''}
                disabled={pendingCard !== null}
                onChange={(e) => void changeProfile(section.card as string, e.target.value)}
              >
                {section.profiles.map((option) => (
                  <option key={option.name} value={option.name}>
                    {option.description}
                    {option.name === PRO_AUDIO_PROFILE ? ' (all inputs)' : ''}
                  </option>
                ))}
              </select>
              <span className={styles.profileCount}>
                {pendingCard === section.card ? 'switching…' : `${section.channels} in`}
              </span>
            </p>
          )}
          {profileError?.card === section.card && (
            <p className={styles.deviceError} role="alert">
              {profileError.message}
            </p>
          )}
          {section.jackCount === 0 && (
            <p className={styles.deviceHint}>
              No input device detected. Plug one in and press Refresh.
            </p>
          )}
          <div
            className={styles.jacks}
            role="listbox"
            aria-label={`Inputs on ${section.title}`}
          >
            {Array.from({ length: section.jackCount }, (_, channel) => {
              const holders = strips.filter(
                (s) =>
                  s.id !== strip.id &&
                  s.input &&
                  (s.input.device ?? null) === section.device &&
                  s.input.device_channel === channel,
              );
              const selected =
                currentDevice === section.device && currentChannel === channel;
              const inUse = holders.length > 0;
              const dead = channel >= section.channels;
              const holderNames = holders.map((s) => s.name).join(', ');
              const label = jackLabel(section.device, channel, letters);
              const state = dead
                ? 'no jack on this device'
                : selected
                  ? 'patched here'
                  : 'free';
              return (
                <button
                  key={channel}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  className={styles.jack}
                  data-selected={selected || undefined}
                  data-in-use={inUse || undefined}
                  data-dead={dead || undefined}
                  aria-label={
                    inUse
                      ? `${label} on ${section.title}, in use by ${holderNames}${selected ? ', patched here' : ''}${dead ? ', no jack on this device' : ''}`
                      : `${label} on ${section.title}, ${state}`
                  }
                  onClick={() => onPatch({ device: section.device, channel })}
                >
                  <span className={styles.socket} aria-hidden>
                    <span className={styles.pin} />
                  </span>
                  <span className={styles.jackLabel}>{label}</span>
                  {inUse ? (
                    <span className={styles.holders}>
                      <span
                        className={styles.linkDot}
                        data-cap={stripeColor(jackKey(section.device, channel))}
                        aria-hidden
                      />
                      {holderNames}
                    </span>
                  ) : (
                    <span className={styles.free}>{dead ? 'no jack' : 'free'}</span>
                  )}
                </button>
              );
            })}
          </div>
        </section>
      ))}
      <footer className={styles.footer}>
        <span className={styles.hint}>
          Patching a jack that's in use links the channels — marked by
          matching tape on both strips.
          {anyProfiles &&
            ' Changing a device’s mode reopens it, so its audio drops for a moment.'}
        </span>
        <ActionButton
          label="Refresh"
          ariaLabel="Rescan audio devices and retry failed ones"
          onPress={() => void refreshDevices()}
        />
        <ActionButton
          label="Disconnect"
          ariaLabel={`Disconnect input from ${strip.name}`}
          onPress={() => onPatch(null)}
          disabled={currentDevice === undefined}
        />
      </footer>
    </Modal>
  );
}
