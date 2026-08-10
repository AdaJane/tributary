import { useRef, useState } from 'react';

import { FADER_MAX_DB, FADER_MIN_DB, clamp, formatDb } from '../audio/db';
import { dbToPosition, positionToDb } from '../audio/taper';
import styles from './Fader.module.css';
import type { CapColor } from './Knob';
import { type DragStart, dragPosition, stepValue } from './knob-interaction';

export interface FaderProps {
  label: string;
  /** Level in dB; at/below the wire floor renders −∞. */
  value: number;
  onChange: (db: number) => void;
  cap: CapColor;
  disabled?: boolean;
}

/** Printed scale, top down. ∞ marks the cut at the very bottom. */
const SCALE_MARKS = [10, 0, -10, -20, -30, -40] as const;

const STEP_DB = 0.5;
const BIG_STEP_DB = 3;

export function Fader({ label, value, onChange, cap, disabled = false }: FaderProps) {
  const position = dbToPosition(value);
  const drag = useRef<DragStart | null>(null);
  const [dragging, setDragging] = useState(false);
  const trackRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const move = (next: number) => {
      e.preventDefault();
      onChange(next);
    };
    switch (e.key) {
      case 'ArrowUp':
      case 'ArrowRight':
        return move(stepValue(value, 1, STEP_DB, FADER_MIN_DB, FADER_MAX_DB));
      case 'ArrowDown':
      case 'ArrowLeft':
        return move(stepValue(value, -1, STEP_DB, FADER_MIN_DB, FADER_MAX_DB));
      case 'PageUp':
        return move(stepValue(value, 1, BIG_STEP_DB, FADER_MIN_DB, FADER_MAX_DB));
      case 'PageDown':
        return move(stepValue(value, -1, BIG_STEP_DB, FADER_MIN_DB, FADER_MAX_DB));
      case 'Home':
        return move(FADER_MAX_DB);
      case 'End':
        return move(FADER_MIN_DB);
      case 'Delete':
        return move(0); // unity
    }
  };

  /** Click on the slot jumps the cap there (console behavior). */
  const jumpTo = (clientY: number) => {
    const track = trackRef.current;
    if (!track) return;
    const rect = track.getBoundingClientRect();
    const t = clamp(1 - (clientY - rect.top) / rect.height, 0, 1);
    onChange(positionToDb(t));
  };

  return (
    <div className={styles.wrap} data-disabled={disabled || undefined}>
      {dragging && <output className={styles.readout}>{formatDb(value)}</output>}
      <div className={styles.scale} aria-hidden>
        {SCALE_MARKS.map((mark) => (
          <span
            key={mark}
            className={styles.mark}
            data-unity={mark === 0 || undefined}
            style={{ bottom: `${dbToPosition(mark) * 100}%` }}
          >
            {mark === 10 ? '+10' : mark}
          </span>
        ))}
        <span className={styles.mark} style={{ bottom: '0%' }}>
          ∞
        </span>
      </div>
      <div
        ref={trackRef}
        className={styles.track}
        onPointerDown={
          disabled
            ? undefined
            : (e) => {
                if (e.target === e.currentTarget) jumpTo(e.clientY);
              }
        }
      >
        <div className={styles.slot} />
        <div
          className={styles.cap}
          role="slider"
          tabIndex={disabled ? -1 : 0}
          aria-label={label}
          aria-orientation="vertical"
          aria-valuemin={FADER_MIN_DB}
          aria-valuemax={FADER_MAX_DB}
          aria-valuenow={value}
          aria-valuetext={formatDb(value)}
          aria-disabled={disabled || undefined}
          data-cap={cap}
          style={{ bottom: `calc(${position} * (100% - var(--fader-cap-h)))` }}
          onKeyDown={disabled ? undefined : onKeyDown}
          onDoubleClick={disabled ? undefined : () => onChange(0)}
          onPointerDown={
            disabled
              ? undefined
              : (e) => {
                  e.stopPropagation();
                  e.currentTarget.setPointerCapture(e.pointerId);
                  drag.current = { position, pointerY: e.clientY };
                  setDragging(true);
                }
          }
          onPointerMove={(e) => {
            if (!drag.current) return;
            onChange(
              positionToDb(dragPosition(drag.current, e.clientY, e.shiftKey)),
            );
          }}
          onPointerUp={(e) => {
            e.currentTarget.releasePointerCapture(e.pointerId);
            drag.current = null;
            setDragging(false);
          }}
        >
          <div className={styles.grip} />
        </div>
      </div>
    </div>
  );
}
