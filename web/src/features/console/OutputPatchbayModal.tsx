import { useEffect, useState } from 'react';

import { ActionButton } from '../../design/ActionButton';
import { InlineError } from '../../design/Panel';
import { Modal } from '../../design/Modal';
import { SegmentedControl } from '../../design/SegmentedControl';
import { SelectField } from '../../design/SelectField';
import { loadAudioStatus, useAudioStatus } from '../../state/audio';
import { useMixer } from '../../state/mixer';
import {
  midiInPorts,
  midiOutPorts,
  midiReports,
  midiRoutes,
  midiTakeTracks,
  refreshMidi,
  routeMidi,
  useMidi,
} from '../../state/midi';
import {
  outputDevices,
  outputPatches,
  patchOutput,
  refreshOutputs,
  useOutputs,
} from '../../state/outputs';
import { useTransport } from '../../state/transport';
import type { InstrumentState } from '../../ws/messages';
import { SourceIcon } from './SourceIcon';
import styles from './OutputPatchbayModal.module.css';
import {
  MIDI_STATUS_LABEL,
  MIDI_TAP_REASON,
  PASS_THROUGH,
  channelConsequence,
  channelOptions,
  groupMidiBay,
  midiSourceFromValue,
  midiSourceLabel,
  midiSourceOptions,
  midiSourceValue,
  trafficLine,
} from './midi-routes';
import {
  ghostReason,
  groupOutputBay,
  jackIsDead,
  jackKey,
  jackLabel,
  outputSilencedReason,
} from './output-devices';
import type { OutputSection } from './output-devices';
import { backendAlert } from './devices';
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

/**
 * What the detail strip is editing.
 *
 * A union rather than two pieces of state, because exactly one thing is
 * selected at a time and two nullable fields would let the room believe an
 * audio jack and a MIDI route are both selected.
 *
 * A MIDI route is identified by INDEX, not by port: a port may carry
 * several routes — that is the merge — so its name does not name one.
 * `index: null` is the row that adds a route to `port`.
 */
type Selected =
  | { kind: 'audio'; device: string | null; channel: number }
  | { kind: 'midi'; port: string; index: number | null };

/** `instruments` is `#[serde(default)]` on the wire, so it is absent before
 * the first snapshot. A module-level empty, never a fresh `[]`: that is the
 * "Maximum update depth exceeded" bug this repo has already been bitten by. */
const NO_INSTRUMENTS: InstrumentState[] = [];

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
  const recording = useTransport((s) => s.phase === 'recording');
  const routes = useMidi(midiRoutes);
  const reports = useMidi(midiReports);
  const outPorts = useMidi(midiOutPorts);
  const inPorts = useMidi(midiInPorts);
  const takeTracks = useMidi(midiTakeTracks);
  const midiPending = useMidi((s) => s.pending);
  const audioStatus = useAudioStatus((s) => s.status);
  const [selected, setSelected] = useState<Selected | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Plug in and reopen is the whole recovery story, exactly as on the
  // input side.
  useEffect(() => {
    if (open) {
      void refreshOutputs();
      void refreshMidi();
      void loadAudioStatus();
    }
  }, [open]);
  useEffect(() => {
    if (!open) {
      setSelected(null);
      setError(null);
    }
  }, [open]);

  const sections = groupOutputBay(devices, patches);
  const backendDown = backendAlert(audioStatus);
  const midiSections = groupMidiBay(routes, reports, outPorts);
  const instruments = mixer.instruments ?? NO_INSTRUMENTS;
  const options = sourceOptions(mixer);
  const midiOptions = midiSourceOptions(instruments, inPorts, takeTracks);
  const audio = selected?.kind === 'audio' ? selected : null;
  const midi = selected?.kind === 'midi' ? selected : null;
  const current = audio ? patchAt(patches, audio.device, audio.channel) : null;
  const currentRoute = midi && midi.index !== null ? routes[midi.index] ?? null : null;

  const apply = async (body: Parameters<typeof patchOutput>[0]) => {
    setError(await patchOutput(body));
  };

  const route = async (body: Parameters<typeof routeMidi>[0]) => {
    const message = await routeMidi(body);
    setError(message);
    return message === null;
  };

  const choose = async (value: string) => {
    if (!audio) return;
    if (value === '') {
      if (current) {
        await apply({
          op: 'unpatch',
          jack: { device: audio.device ?? undefined, channel: audio.channel },
        });
      }
      return;
    }
    const option = options.find((o) => o.value === value);
    if (!option) return;
    await apply({
      op: 'patch',
      patch: {
        device: audio.device ?? undefined,
        channel: audio.channel,
        source: option.source,
        source_channel: option.channel,
        tap: current?.tap ?? 'pre_fader',
      },
    });
  };

  /**
   * Point the selected MIDI row at `value`.
   *
   * A route's identity is (port, source), so changing the source is not an
   * edit — it is a different route. Two calls, old one first: if the
   * unroute fails there is nothing to replace it with, and stopping leaves
   * the document as the user last saw it.
   */
  const chooseMidi = async (value: string) => {
    if (!midi) return;
    if (currentRoute) {
      const gone = await route({
        op: 'unroute',
        port: currentRoute.port,
        source: currentRoute.source,
      });
      if (!gone) return;
    }
    if (value === '') {
      setSelected({ kind: 'midi', port: midi.port, index: null });
      return;
    }
    const source = midiSourceFromValue(value, instruments, inPorts, takeTracks);
    if (!source) return;
    if (await route({ op: 'route', route: { port: midi.port, channel: null, source } })) {
      // Found by identity in the document that just came back, never by
      // arithmetic on the one we had: an unroute renumbers everything
      // after it, and a stale index would edit somebody else's route.
      const fresh = useMidi.getState().document?.routes ?? [];
      const index = fresh.findIndex(
        (r) => r.port === midi.port && midiSourceValue(r.source) === midiSourceValue(source),
      );
      setSelected({ kind: 'midi', port: midi.port, index: index < 0 ? null : index });
    }
  };

  return (
    <Modal title="Patch outputs" open={open} onClose={onClose}>
      {!supported && (
        <p className={styles.unsupported} role="status">
          This audio backend has no patchable outputs. The monitor still
          plays; nothing else can be routed out.
        </p>
      )}
      {backendDown && (
        <p className={styles.deviceError} role="alert">
          {backendDown}
        </p>
      )}
      {sections.length === 0 && supported && !backendDown && (
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
                  audio?.device === section.device && audio.channel === channel;
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
                      setSelected({ kind: 'audio', device: section.device, channel });
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

      {midiSections.length > 0 && (
        <p className={styles.divider}>
          MIDI outputs carry notes, not audio — so they take their own
          sources, and unlike a direct out they may be re-routed while the
          tape rolls.
        </p>
      )}
      {midiSections.map((section) => {
        const traffic = trafficLine(section);
        return (
          <section key={section.port} className={styles.deviceSection}>
            <p className={styles.device}>
              <span className={styles.chip}>MIDI</span>
              <span className={styles.deviceIcon}>
                <SourceIcon kind="midi" />
              </span>
              <span className={styles.deviceName}>{section.title}</span>
              <span
                className={styles.status}
                data-status={section.absent ? 'absent' : 'open'}
              >
                <span className={styles.statusDot} aria-hidden />
                {section.absent ? 'missing' : 'connected'}
              </span>
            </p>
            {section.absent && (
              <p className={styles.ghost}>
                this MIDI port is not connected — route these somewhere else
              </p>
            )}
            {traffic && (
              <p className={section.errors > 0 ? styles.deviceError : styles.traffic} role={section.errors > 0 ? 'alert' : undefined}>
                {traffic}
              </p>
            )}
            <div className={styles.routes} role="listbox" aria-label={`${section.title} routes`}>
              {section.rows.map((row) => {
                const isSelected = midi?.index === row.index;
                const label = midiSourceLabel(row.route.source, instruments);
                return (
                  <button
                    key={`${row.index}`}
                    type="button"
                    role="option"
                    aria-selected={isSelected}
                    className={styles.route}
                    data-selected={isSelected || undefined}
                    aria-label={
                      `${label} to ${section.title}, ${MIDI_STATUS_LABEL[row.status]}` +
                      `${row.reason ? `, ${row.reason}` : ''}`
                    }
                    onClick={() =>
                      setSelected({ kind: 'midi', port: section.port, index: row.index })
                    }
                  >
                    <span className={styles.routeStatus} data-status={row.status} aria-hidden />
                    <span className={styles.routeLabel}>{label}</span>
                    <span className={styles.routeChannel}>
                      {row.route.channel == null ? 'thru' : `ch ${row.route.channel + 1}`}
                    </span>
                    {row.reason && <span className={styles.routeReason}>{row.reason}</span>}
                  </button>
                );
              })}
              <button
                type="button"
                role="option"
                aria-selected={midi?.port === section.port && midi.index === null}
                className={styles.addRoute}
                data-selected={
                  (midi?.port === section.port && midi.index === null) || undefined
                }
                aria-label={`Add a route to ${section.title}`}
                onClick={() => setSelected({ kind: 'midi', port: section.port, index: null })}
              >
                + route
              </button>
            </div>
          </section>
        );
      })}

      <div className={styles.detail}>
        {selected === null && (
          <p className={styles.hint}>Select an output or a MIDI route above to patch it.</p>
        )}
        {audio && (
          <>
            {recording && (
              <p className={styles.locked} role="status">
                Locked while recording: re-patching rebuilds the mix graph,
                which would cut every reverb tail onto the tape. Pre/post
                still moves.
              </p>
            )}
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
              disabled={pending || !supported || recording}
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
                      device: audio.device ?? undefined,
                      channel: audio.channel,
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
        {midi && (
          <>
            <SelectField
              label="Source"
              value={currentRoute ? midiSourceValue(currentRoute.source) : ''}
              options={[{ value: '', label: '— none —' }, ...midiOptions]}
              disabled={midiPending}
              hint={midiOptions.length === 0 ? 'nothing to send' : undefined}
              onChange={(value) => void chooseMidi(value)}
            />
            <SelectField
              label="Channel"
              value={
                currentRoute?.channel == null ? PASS_THROUGH : String(currentRoute.channel)
              }
              options={channelOptions()}
              disabled={midiPending || currentRoute === null}
              onChange={(value) => {
                if (!currentRoute) return;
                void route({
                  op: 'route',
                  route: {
                    ...currentRoute,
                    channel: value === PASS_THROUGH ? null : Number(value),
                  },
                });
              }}
            />
            <span className={styles.consequence}>
              {channelConsequence(currentRoute?.channel ?? null)}
            </span>
            <div className={styles.tap}>
              {/* Disabled with its reason rather than hidden: "why does the
                  vocal have a pre/post switch and the Juno not" is a fair
                  question, and a control that vanishes teaches nothing. */}
              <SegmentedControl
                label="Tap"
                options={TAPS}
                value="pre_fader"
                disabled
                onChange={() => {}}
              />
              <span className={styles.consequence}>{MIDI_TAP_REASON}</span>
            </div>
          </>
        )}
        {error && <InlineError message={error} />}
      </div>

      <footer className={styles.footer}>
        <span className={styles.hint}>
          One audio output carries one source, so patching an output already
          in use replaces what was there. A MIDI port takes any number —
          that is the merge.
        </span>
        <ActionButton
          label="Refresh"
          ariaLabel="Rescan audio and MIDI outputs and retry failed ones"
          onPress={() => {
            void refreshOutputs();
            void refreshMidi();
          }}
        />
        <ActionButton
          label="Disconnect"
          ariaLabel="Unpatch the selected output or MIDI route"
          disabled={
            (current === null && currentRoute === null) ||
            pending ||
            midiPending ||
            (audio !== null && recording)
          }
          onPress={() => {
            if (audio) {
              void apply({
                op: 'unpatch',
                jack: { device: audio.device ?? undefined, channel: audio.channel },
              });
            } else if (midi && currentRoute) {
              void route({
                op: 'unroute',
                port: currentRoute.port,
                source: currentRoute.source,
              }).then(
                (gone) => gone && setSelected({ kind: 'midi', port: midi.port, index: null }),
              );
            }
          }}
        />
      </footer>
    </Modal>
  );
}
