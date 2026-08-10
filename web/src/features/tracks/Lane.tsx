import { useEffect, useRef } from 'react';

import { $api } from '../../api/client';
import { PushButton } from '../../design/PushButton';
import { waveformColumns } from './waveform-path';
import styles from './Lane.module.css';

export interface LaneProps {
  name: string;
  isMaster: boolean;
  laneIndex: number;
  solo: boolean;
  mute: boolean;
  damaged: boolean;
  pairs: Int16Array;
  samplesPerBin: number;
  fpp: number;
  startFrame: number;
  widthPx: number;
}

/** One track: printed header (name + playback SOLO/MUTE) + the visible
 * slice of its waveform. The canvas is viewport-sized; scrolling redraws
 * rather than growing DOM. */
export function Lane({
  name,
  isMaster,
  laneIndex,
  solo,
  mute,
  damaged,
  pairs,
  samplesPerBin,
  fpp,
  startFrame,
  widthPx,
}: LaneProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

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
    const ink = style.getPropertyValue('--wave-ink').trim();
    const dim = style.getPropertyValue('--wave-ink-dim').trim();
    ctx.clearRect(0, 0, widthPx, height);

    const mid = height / 2;
    ctx.strokeStyle = dim;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, mid + 0.5);
    ctx.lineTo(widthPx, mid + 0.5);
    ctx.stroke();

    const columns = waveformColumns(pairs, samplesPerBin, fpp, startFrame, widthPx);
    const half = mid - 2;
    ctx.strokeStyle = ink;
    ctx.beginPath();
    for (let x = 0; x < widthPx; x += 1) {
      const min = columns[x * 2];
      const max = columns[x * 2 + 1];
      if (min === 0 && max === 0) continue;
      ctx.moveTo(x + 0.5, mid - max * half);
      ctx.lineTo(x + 0.5, mid - min * half + 1);
    }
    ctx.stroke();
  }, [pairs, samplesPerBin, fpp, startFrame, widthPx]);

  return (
    <div className={styles.lane}>
      <div className={styles.header} data-master={isMaster || undefined}>
        <span className={styles.name}>
          {name}
          {damaged && (
            <span className={styles.damaged} title="Dropped samples were padded with silence">
              DAMAGED
            </span>
          )}
        </span>
        <div className={styles.gates}>
          <PushButton
            label="Solo"
            ariaLabel={`Solo ${name} for playback`}
            variant="solo"
            pressed={solo}
            onToggle={(on) =>
              void $api.PUT('/api/v1/transport/lanes/{index}', {
                params: { path: { index: laneIndex } },
                body: { solo: on },
              })
            }
          />
          <PushButton
            label="Mute"
            ariaLabel={`Mute ${name} for playback`}
            variant="mute"
            pressed={mute}
            onToggle={(on) =>
              void $api.PUT('/api/v1/transport/lanes/{index}', {
                params: { path: { index: laneIndex } },
                body: { mute: on },
              })
            }
          />
        </div>
      </div>
      <canvas
        ref={canvasRef}
        className={styles.wave}
        style={{ width: widthPx }}
        role="img"
        aria-label={`Waveform of ${name}`}
      />
    </div>
  );
}
