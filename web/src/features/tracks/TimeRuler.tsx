import { useEffect, useRef, useState } from 'react';

import type { LoopRegion } from '../../state/transport';
import { rulerGesture } from './loop-drag';
import { tickStepSecs } from './timeline';
import styles from './TimeRuler.module.css';

export interface TimeRulerProps {
  fpp: number;
  startFrame: number;
  widthPx: number;
  sampleRate: number;
  totalFrames: number;
  loop: LoopRegion | null;
  onSeek: (frame: number) => void;
  onLoop: (startFrames: number, endFrames: number) => void;
}

function tickLabel(frame: number, sampleRate: number, stepSecs: number): string {
  const totalSeconds = frame / sampleRate;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds - minutes * 60;
  if (stepSecs < 1) {
    return `${minutes}:${seconds.toFixed(1).padStart(4, '0')}`;
  }
  return `${minutes}:${String(Math.round(seconds)).padStart(2, '0')}`;
}

/** The timeline ruler: printed ticks, click to seek, drag to loop. */
export function TimeRuler({
  fpp,
  startFrame,
  widthPx,
  sampleRate,
  totalFrames,
  loop,
  onSeek,
  onLoop,
}: TimeRulerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const dragStart = useRef<number | null>(null);
  /** Live drag preview in local px, redrawn while painting a region. */
  const [preview, setPreview] = useState<[number, number] | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || widthPx <= 0) return;
    const dpr = window.devicePixelRatio || 1;
    const height = canvas.clientHeight;
    canvas.width = Math.round(widthPx * dpr);
    canvas.height = Math.round(height * dpr);
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.scale(dpr, dpr);

    const style = getComputedStyle(canvas);
    const dim = style.getPropertyValue('--text-dim').trim();
    const label = style.getPropertyValue('--text-label').trim();
    const region = style.getPropertyValue('--loop-region').trim();
    const regionEdge = style.getPropertyValue('--playhead-color').trim();
    ctx.clearRect(0, 0, widthPx, height);

    // The engaged loop, and the region being painted, shade the ruler.
    const shade = (aPx: number, bPx: number) => {
      ctx.fillStyle = region;
      ctx.fillRect(aPx, 0, bPx - aPx, height);
      ctx.strokeStyle = regionEdge;
      for (const x of [aPx, bPx]) {
        ctx.beginPath();
        ctx.moveTo(x + 0.5, 0);
        ctx.lineTo(x + 0.5, height);
        ctx.stroke();
      }
    };
    if (loop) {
      shade((loop.startFrames - startFrame) / fpp, (loop.endFrames - startFrame) / fpp);
    }
    if (preview) {
      shade(Math.min(...preview), Math.max(...preview));
    }

    ctx.font = `9px ${style.getPropertyValue('--font-mono').trim() || 'monospace'}`;
    const stepSecs = tickStepSecs(fpp, sampleRate);
    const stepFrames = stepSecs * sampleRate;
    const first = Math.ceil(startFrame / stepFrames) * stepFrames;
    const lastFrame = Math.min(startFrame + widthPx * fpp, totalFrames);
    for (let frame = first; frame <= lastFrame; frame += stepFrames) {
      const x = Math.round((frame - startFrame) / fpp) + 0.5;
      ctx.strokeStyle = dim;
      ctx.beginPath();
      ctx.moveTo(x, height - 8);
      ctx.lineTo(x, height);
      ctx.stroke();
      ctx.fillStyle = label;
      ctx.fillText(tickLabel(frame, sampleRate, stepSecs), x + 3, height - 10);
    }
  }, [fpp, startFrame, widthPx, sampleRate, totalFrames, loop, preview]);

  const localX = (e: React.PointerEvent<HTMLCanvasElement>) =>
    e.clientX - e.currentTarget.getBoundingClientRect().left;

  return (
    <canvas
      ref={canvasRef}
      className={styles.ruler}
      style={{ width: widthPx }}
      aria-label="Timeline ruler — click to move the playhead, drag to loop"
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        dragStart.current = localX(e);
      }}
      onPointerMove={(e) => {
        if (dragStart.current === null) return;
        setPreview([dragStart.current, localX(e)]);
      }}
      onPointerUp={(e) => {
        const down = dragStart.current;
        dragStart.current = null;
        setPreview(null);
        if (down === null) return;
        const gesture = rulerGesture(down, localX(e), fpp, startFrame, totalFrames);
        if (gesture.kind === 'seek') onSeek(gesture.frame);
        else onLoop(gesture.startFrames, gesture.endFrames);
      }}
      onPointerCancel={() => {
        dragStart.current = null;
        setPreview(null);
      }}
    />
  );
}
