import { useEffect, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { InlineError } from '../../design/Panel';
import { Modal } from '../../design/Modal';
import { SegmentedControl } from '../../design/SegmentedControl';
import { SelectField } from '../../design/SelectField';
import { useMixer } from '../../state/mixer';
import {
  outputDevices,
  outputPatches,
  patchOutput,
  refreshOutputs,
  useOutputs,
} from '../../state/outputs';
import { SourceIcon } from './SourceIcon';
import styles from './OutputPatchbayModal.module.css';
import {
  ghostReason,
  groupOutputBay,
  jackIsDead,
  jackKey,
  jackLabel,
  outputSilencedReason,
} from './output-devices';
import type { OutputSection } from './output-devices';
import { patchAt } from './output-patch';
import type { OutputSource, PatchTap } from './output-patch';
import { sourceChannelName, sourceOptions, tapConsequence } from './output-sources';

const STATUS_LABEL: Record<OutputSection['status'], string> = {
  open: 'live',
  available: 'ready',
  failed: 'failed',
  absent: 'missing',
};

const TAPS: { value: PatchTap; label: string }[] = [
  { value: 'pre_fader', label: 'Pre' },
  { value: 'post_fader', label: 'Post' },
];

interface Selected {
  device: string | null;
  channel: number;
}

/**
 * The output patch bay, organised BY OUTPUT.
 *
 * The cardinality is inverted from the input room: there, a strip is the
 * "one" and jacks are the "many", so it opens from the one and lists the
 * many. Here an output channel is the "one" — it holds at most one feed —
 * so the room lists outputs and picks a source. That single decision is
 * also what makes the master and the buses reachable: they appear in the
 * source list rather than needing channel strips they do not have.
 */
export function OutputPatchbayModal({
  open,
  onClose,
  initialSource,
}: {
  open: boolean;
  onClose: () => void;
  /** Pre-fills the source when opened from a strip or the master. */
  initialSource?: OutputSource;
}) {
  const mixer = useMixer((s) => s.state);
  const patches = useOutputs(outputPatches);
  const devices = useOutputs(outputDevices);
  const supported = useOutputs((s) => s.document?.supported ?? true);
  const pending = useOutputs((s) => s.pending);
  const [selected, setSelected] = useState<Selected | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Plug in and reopen is the whole recovery story, exactly as on the
  // input side.
  useEffect(() => {
    if (open) void refreshOutputs();
  }, [open]);
  useEffect(() => {
    if (!open) {
      setSelected(null);
      setError(null);
    }
  }, [open]);

  const sections = groupOutputBay(devices, patches);
  const options = sourceOptions(mixer);
  const current = selected ? patchAt(patches, selected.device, selected.channel) : null;

  const apply = async (body: Parameters<typeof patchOutput>[0]) => {
    setError(await patchOutput(body));
  };

  const choose = async (value: string) => {
    if (!selected) return;
    if (value === '') {
      if (current) {
        await apply({
          op: 'unpatch',
          jack: { device: selected.device ?? undefined, channel: selected.channel },
        });
      }
      return;
    }
    const option = options.find((o) => o.value === value);
    if (!option) return;
    await apply({
      op: 'patch',
      patch: {
        device: selected.device ?? undefined,
        channel: selected.channel,
        source: option.source,
        source_channel: option.channel,
        tap: current?.tap ?? 'pre_fader',
      },
    });
  };

  return (
    <Modal title="Patch outputs" open={open} onClose={onClose}>
      {!supported && (
        <p className={styles.unsupported} role="status">
          This audio backend has no patchable outputs. The monitor still
          plays; nothing else can be routed out.
        </p>
      )}
      {sections.length === 0 && supported && (
        <p className={styles.empty}>
          No audio output device detected. Plug one in and press Refresh.
        </p>
      )}
      {sections.map((section) => {
        const silenced = outputSilencedReason(section);
        const ghost = ghostReason(section);
        return (
          <section key={section.device ?? ''} className={styles.deviceSection}>
            <p className={styles.device}>
              <span className={styles.chip}>OUT</span>
              <span className={styles.deviceIcon}>
                <SourceIcon kind={section.device === null ? 'default' : 'line'} />
              </span>
              <span className={styles.deviceName}>{section.title}</span>
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
            {ghost && <p className={styles.ghost}>{ghost}</p>}
            {silenced && <p className={styles.silenced}>Silent at the output: {silenced}</p>}
            <div className={styles.jacks} role="listbox" aria-label={`${section.title} outputs`}>
              {Array.from({ length: section.jackCount }, (_, channel) => {
                const patch = patchAt(patches, section.device, channel);
                const dead = jackIsDead(section, channel);
                const isSelected =
                  selected?.device === section.device && selected.channel === channel;
                const holder = patch
                  ? sourceChannelName(patch.source, patch.source_channel, mixer)
                  : null;
                return (
                  <button
                    key={jackKey(section, channel)}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className={styles.jack}
                    data-selected={isSelected || undefined}
                    data-patched={patch ? true : undefined}
                    data-dead={dead || undefined}
                    aria-label={
                      `${jackLabel(section, channel)} on ${section.title}, ` +
                      `${holder ? `fed by ${holder}` : 'free'}` +
                      `${dead ? ', no jack on this device' : ''}` +
                      `${section.status === 'failed' ? ', device failed' : ''}`
                    }
                    onClick={() => {
                      setSelected({ device: section.device, channel });
                      // Opened from a strip or the master, a FREE output
                      // patches on the spot: two informed clicks, OUT then
                      // tile. An occupied one only selects, so its holder
                      // is read before it is replaced.
                      if (initialSource && !patch && !dead && supported) {
                        void apply({
                          op: 'patch',
                          patch: {
                            device: section.device ?? undefined,
                            channel,
                            source: initialSource,
                            source_channel: 0,
                            tap: 'pre_fader',
                          },
                        });
                      }
                    }}
                  >
                    <span className={styles.socket} aria-hidden>
                      <span className={styles.pin} />
                      <span className={styles.pin} />
                      <span className={styles.pin} />
                    </span>
                    <span className={styles.jackLabel}>{jackLabel(section, channel)}</span>
                    {holder ? (
                      <span className={styles.holder}>{holder}</span>
                    ) : (
                      <span className={styles.free}>{dead ? 'no jack' : 'free'}</span>
                    )}
                  </button>
                );
              })}
            </div>
          </section>
        );
      })}

      <div className={styles.detail}>
        {selected === null ? (
          <p className={styles.hint}>Select an output above to patch it.</p>
        ) : (
          <>
            <SelectField
              label="Source"
              value={
                current
                  ? options.find(
                      (o) =>
                        o.channel === current.source_channel &&
                        JSON.stringify(o.source) === JSON.stringify(current.source),
                    )?.value ?? ''
                  : ''
              }
              options={[{ value: '', label: '— none —' }, ...options]}
              disabled={pending || !supported}
              onChange={(value) => void choose(value)}
            />
            <div className={styles.tap}>
              <SegmentedControl
                label="Tap"
                options={TAPS}
                value={current?.tap ?? 'pre_fader'}
                disabled={pending || current === null}
                onChange={(tap) =>
                  void apply({
                    op: 'tap',
                    jack: {
                      device: selected.device ?? undefined,
                      channel: selected.channel,
                    },
                    tap,
                  })
                }
              />
              <span className={styles.consequence}>
                {tapConsequence(current?.tap ?? 'pre_fader')}
              </span>
            </div>
          </>
        )}
        {error && <InlineError message={error} />}
      </div>

      <footer className={styles.footer}>
        <span className={styles.hint}>
          One output carries one source. Patching an output that is already
          in use replaces what was there.
        </span>
        <ActionButton
          label="Refresh"
          ariaLabel="Rescan audio outputs and retry failed ones"
          onPress={() => void refreshOutputs()}
        />
        <ActionButton
          label="Disconnect"
          ariaLabel="Unpatch the selected output"
          disabled={current === null || pending}
          onPress={() =>
            selected &&
            void apply({
              op: 'unpatch',
              jack: { device: selected.device ?? undefined, channel: selected.channel },
            })
          }
        />
      </footer>
    </Modal>
  );
}
