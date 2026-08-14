import { useState } from 'react';

import { $api } from '../../api/client';
import { useDevices } from '../../state/devices';
import { useInstruments } from '../../state/instruments';
import { useMixer } from '../../state/mixer';
import type { InstrumentState, StripState } from '../../ws/messages';
import styles from './InputButton.module.css';
import { InputPickerModal } from './InputPickerModal';
import { groupPatchbay } from './devices';
import { inputSource, toBody } from './input-source';
import type { InputSource } from './input-source';
import { patchDescription } from './instrument-logic';
import { deviceLetters, sourceLabel } from './linked-inputs';

/** The strip-top INPUT control: shows the current patch ("IN 3" on the
 * default box, "B2" on stage box B, "INST 1.1" on a rack unit), and opens
 * the patchbay to change it. */
const NO_INSTRUMENTS: readonly InstrumentState[] = [];

export function InputButton({ strip }: { strip: StripState }) {
  const [open, setOpen] = useState(false);
  const strips = useMixer((s) => s.state.strips);
  const devices = useDevices((s) => s.devices);
  const instruments = useMixer((s) => s.state.instruments) ?? NO_INSTRUMENTS;
  // Only for the aria sentence: the button prints its slot, but a screen
  // reader should hear the instrument's name, not its number.
  useInstruments((s) => s.loaded);

  const letters = deviceLetters(
    groupPatchbay(devices, strips)
      .filter((s) => s.device !== null)
      .map((s) => s.device as string),
  );
  const current = inputSource(strip.input);
  const label = sourceLabel(current, letters);
  const ariaPatch = patchDescription(current, letters, instruments);

  const patch = (target: InputSource | null) => {
    void $api.PUT('/api/v1/strips/{id}', {
      params: { path: { id: strip.id } },
      body: { input: toBody(target) },
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
