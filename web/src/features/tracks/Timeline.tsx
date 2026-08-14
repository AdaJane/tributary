import { useCallback, useEffect, useRef, useState } from 'react';

import { $api } from '../../api/client';
import { ActionButton } from '../../design/ActionButton';
import { useMixer } from '../../state/mixer';
import { usePeaksStore } from '../../state/peaks';
import { useTransport } from '../../state/transport';
import { Lane } from './Lane';
import { Playhead } from './Playhead';
import { TimeRuler } from './TimeRuler';
import { laneName } from './lane-name';
import { RECORD_FPP, clampFpp, fitFpp, totalPx } from './timeline';
import { followScroll } from './follow';
import styles from './Timeline.module.css';

/**
 * The multitrack editor surface. The view is VIRTUAL: an absolutely
 * positioned spacer gives the scroller its width, while a sticky pane
 * holds viewport-sized canvases redrawn from the scroll position — so an
 * hour-long take never meets a canvas size limit.
 */
export function Timeline() {
  const samplesPerBin = usePeaksStore((s) => s.samplesPerBin);
  const tracks = usePeaksStore((s) => s.tracks);
  const trackMeta = usePeaksStore((s) => s.trackMeta);
  const strips = useMixer((s) => s.state.strips);
  const takeFrames = useTransport((s) => s.totalFrames);
  const sampleRate = useTransport((s) => s.sampleRate);
  const playing = useTransport((s) => s.phase === 'playing');
  const recording = useTransport((s) => s.phase === 'recording');
  const lanes = useTransport((s) => s.lanes);
  const loop = useTransport((s) => s.loop);
  const liveFrames = usePeaksStore((s) => s.liveFrames);
  // While recording, the timeline is the growing take itself.
  const totalFrames = recording ? liveFrames : takeFrames;

  const scrollerRef = useRef<HTMLDivElement>(null);
  const [viewWidth, setViewWidth] = useState(0);
  const [scrollLeft, setScrollLeft] = useState(0);
  /** null = fit the whole take. */
  const [fpp, setFpp] = useState<number | null>(null);
  const followArmed = useRef(true);
  const programmatic = useRef(false);
  const headerW = useRef(148);

  useEffect(() => {
    const raw = getComputedStyle(document.documentElement).getPropertyValue('--lane-header-w');
    const parsed = Number.parseFloat(raw);
    if (!Number.isNaN(parsed)) headerW.current = parsed;
  }, []);

  const hasContent = tracks.length > 0 && (totalFrames > 0 || recording);
  useEffect(() => {
    // Re-runs when the scroller first renders — the component returns null
    // until peaks arrive, so a mount-only effect would observe nothing.
    if (!hasContent) return;
    const el = scrollerRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setViewWidth(el.clientWidth));
    observer.observe(el);
    setViewWidth(el.clientWidth);
    return () => observer.disconnect();
  }, [hasContent]);

  // A fresh PLAY or REC re-arms auto-follow even after a manual scroll.
  useEffect(() => {
    if (playing || recording) followArmed.current = true;
  }, [playing, recording]);

  const waveW = Math.max(0, viewWidth - headerW.current);
  // Recording pins the zoom: fit would rescale as the take grows.
  const effFpp = recording
    ? (fpp ?? RECORD_FPP)
    : clampFpp(fpp ?? fitFpp(totalFrames, waveW), totalFrames, waveW);
  const contentW = totalPx(totalFrames, effFpp);
  const startFrame = scrollLeft * effFpp;

  const requestScroll = useCallback((x: number) => {
    const el = scrollerRef.current;
    if (!el) return;
    programmatic.current = true;
    el.scrollLeft = x;
    setScrollLeft(x);
  }, []);

  const zoomTo = useCallback(
    (next: number) => {
      const clamped = clampFpp(next, totalFrames, waveW);
      const centerFrame = (scrollLeft + waveW / 2) * effFpp;
      setFpp(clamped);
      requestScroll(Math.max(0, centerFrame / clamped - waveW / 2));
    },
    [totalFrames, waveW, scrollLeft, effFpp, requestScroll],
  );

  const seek = useCallback((frame: number) => {
    followArmed.current = true;
    void $api.POST('/api/v1/transport/seek', {
      body: { position_frames: Math.round(frame) },
    });
  }, []);

  const setLoop = useCallback((startFrames: number, endFrames: number) => {
    void $api.PUT('/api/v1/transport/loop', {
      body: { start_frames: startFrames, end_frames: endFrames },
    });
  }, []);

  const onPlayheadTick = useCallback(
    (contentPx: number) => {
      if (!followArmed.current) return;
      const el = scrollerRef.current;
      if (!el) return;
      const next = followScroll(contentPx, el.scrollLeft, waveW, contentW);
      if (next !== null) requestScroll(next);
    },
    [waveW, contentW, requestScroll],
  );

  const getScrollLeft = useCallback(() => scrollerRef.current?.scrollLeft ?? 0, []);

  if (!hasContent) return null;

  return (
    <div
      className={styles.scroller}
      ref={scrollerRef}
      onScroll={() => {
        const el = scrollerRef.current;
        if (!el) return;
        // A scroll the user initiated disarms follow; ours does not.
        if (programmatic.current) programmatic.current = false;
        else followArmed.current = false;
        setScrollLeft(el.scrollLeft);
      }}
    >
      <div className={styles.spacer} style={{ width: contentW + headerW.current }} />
      <div className={styles.pane}>
        <div className={styles.rulerRow}>
          <div className={styles.corner}>
            <ActionButton label="−" ariaLabel="Zoom out" onPress={() => zoomTo(effFpp * 2)} />
            <ActionButton label="+" ariaLabel="Zoom in" onPress={() => zoomTo(effFpp / 2)} />
            <ActionButton
              label="Fit"
              ariaLabel="Fit the whole take"
              onPress={() => {
                setFpp(null);
                requestScroll(0);
              }}
            />
          </div>
          <TimeRuler
            fpp={effFpp}
            startFrame={startFrame}
            widthPx={waveW}
            sampleRate={sampleRate}
            totalFrames={totalFrames}
            loop={loop}
            onSeek={seek}
            onLoop={setLoop}
          />
        </div>
        {tracks.map((pairs, i) => (
          /* Lanes are positional (index-aligned with the take's tracks);
             meta files can collide while live (unnamed), so never key on
             them. */
          <Lane
            key={i}
            name={laneName(trackMeta[i], strips)}
            isMaster={trackMeta[i]?.channels === 2}
            laneIndex={i}
            solo={lanes[i]?.solo ?? false}
            mute={lanes[i]?.mute ?? false}
            damaged={trackMeta[i]?.damaged ?? false}
            midi={trackMeta[i]?.midi ?? false}
            pairs={pairs}
            samplesPerBin={samplesPerBin}
            fpp={effFpp}
            startFrame={startFrame}
            widthPx={waveW}
          />
        ))}
        <Playhead
          fpp={effFpp}
          headerW={headerW.current}
          getScrollLeft={getScrollLeft}
          viewWidth={waveW}
          onTick={onPlayheadTick}
        />
      </div>
    </div>
  );
}
