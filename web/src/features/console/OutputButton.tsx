import { useState } from 'react';

import { useMixer } from '../../state/mixer';
import { outputPatches, useOutputs } from '../../state/outputs';
import styles from './OutputButton.module.css';
import { OutputPatchbayModal } from './OutputPatchbayModal';
import type { OutputSource } from './output-patch';
import { outputButtonLabel, outputDescription } from './output-sources';

/**
 * The OUT control: prints where this source goes ("—", "OUT 3", "2 OUTS")
 * and opens the output patch bay to change it.
 *
 * Takes a `source` rather than a strip, so the master section can use the
 * same component — the master has no channel strip, and it is exactly the
 * thing a strip-only door could never reach.
 */
export function OutputButton({ source, name }: { source: OutputSource; name: string }) {
  const [open, setOpen] = useState(false);
  const patches = useOutputs(outputPatches);
  const mixer = useMixer((s) => s.state);

  return (
    <div className={styles.wrap}>
      <span className={styles.caption}>Output</span>
      <button
        type="button"
        className={styles.button}
        aria-haspopup="dialog"
        aria-label={`Output for ${name}: ${outputDescription(patches, source, mixer)} — open the output patch bay`}
        onClick={() => setOpen(true)}
      >
        {outputButtonLabel(patches, source)}
      </button>
      <OutputPatchbayModal
        open={open}
        onClose={() => setOpen(false)}
        initialSource={source}
      />
    </div>
  );
}
