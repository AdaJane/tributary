import { useState } from 'react';

import { $api } from '../../api/client';
import { useDevices } from '../../state/devices';
import { useMixer } from '../../state/mixer';
import type { StripState } from '../../ws/messages';
import styles from './InputButton.module.css';
import { InputPickerModal, type PatchTarget } from './InputPickerModal';
import { groupPatchbay } from './devices';
import { deviceLetters, jackLabel } from './linked-inputs';

/** The strip-top INPUT control: shows the current patch ("IN 3" on the
 * default box, "B2" on stage box B), opens the patchbay to change it. */
export function InputButton({ strip }: { strip: StripState }) {
  const [open, setOpen] = useState(false);
  const strips = useMixer((s) => s.state.strips);
  const devices = useDevices((s) => s.devices);

  const letters = deviceLetters(
    groupPatchbay(devices, strips)
      .filter((s) => s.device !== null)
      .map((s) => s.device as string),
  );
  const current = strip.input
    ? {
        device: strip.input.device ?? null,
        channel: strip.input.device_channel,
      }
    : null;
  const label = current === null ? '—' : jackLabel(current.device, current.channel, letters);
  const ariaPatch =
    current === null
      ? 'none'
      : `${jackLabel(current.device, current.channel, letters)}${
          current.device ? ` on ${current.device}` : ''
        }`;

  const patch = (target: PatchTarget | null) => {
    void $api.PUT('/api/v1/strips/{id}', {
      params: { path: { id: strip.id } },
      body: {
        input: target === null ? null : { device: target.device, channel: target.channel },
      },
    });
    setOpen(false);
  };

  return (
    <div className={styles.wrap}>
      <span className={styles.caption}>Input</span>
      <button
        type="button"
        className={styles.button}
        aria-haspopup="dialog"
        aria-label={`Input for ${strip.name}: ${ariaPatch} — open patchbay`}
        onClick={() => setOpen(true)}
      >
        {label}
      </button>
      <InputPickerModal
        strip={strip}
        strips={strips}
        open={open}
        onClose={() => setOpen(false)}
        onPatch={patch}
      />
    </div>
  );
}
