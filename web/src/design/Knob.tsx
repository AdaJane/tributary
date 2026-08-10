import { useRef, useState } from 'react';

import { clamp } from '../audio/db';
import styles from './Knob.module.css';
import {
  type DragStart,
  dragPosition,
  positionToAngle,
  stepValue,
} from './knob-interaction';

export type CapColor = 'red' | 'blue' | 'yellow' | 'green' | 'white' | 'grey';

export interface KnobProps {
  /** Printed under the knob and the base of the aria label. */
  label: string;
  value: number;
  min: number;
  max: number;
  /** Double-click / Delete restore this. */
  defaultValue: number;
  onChange: (value: number) => void;
  /** Human reading for aria-valuetext and the drag readout. */
  format: (value: number) => string;
  /** Domain↔position mapping; linear when omitted (log sweeps pass one). */
  toPosition?: (value: number) => number;
  fromPosition?: (position: number) => number;
  /** Arrow-key step in domain units; PageUp/Down = `bigStep`. */
  step?: number;
  bigStep?: number;
  cap: CapColor;
  disabled?: boolean;
}

const TICK_COUNT = 11;

export function Knob({
  label,
  value,
  min,
  max,
  defaultValue,
  onChange,
  format,
  toPosition,
  fromPosition,
  step = 1,
  bigStep = 5,
  cap,
  disabled = false,
}: KnobProps) {
  const linearTo = (v: number) => (v - min) / (max - min);
  const linearFrom = (t: number) => min + t * (max - min);
  const position = clamp((toPosition ?? linearTo)(value), 0, 1);
  const fromPos = fromPosition ?? linearFrom;

  const drag = useRef<DragStart | null>(null);
  const [dragging, setDragging] = useState(false);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const move = (next: number) => {
      e.preventDefault();
      onChange(next);
    };
    switch (e.key) {
      case 'ArrowUp':
      case 'ArrowRight':
        return move(stepValue(value, 1, step, min, max));
      case 'ArrowDown':
      case 'ArrowLeft':
        return move(stepValue(value, -1, step, min, max));
      case 'PageUp':
        return move(stepValue(value, 1, bigStep, min, max));
      case 'PageDown':
        return move(stepValue(value, -1, bigStep, min, max));
      case 'Home':
        return move(min);
      case 'End':
        return move(max);
      case 'Delete':
        return move(defaultValue);
    }
  };

  return (
    <div className={styles.wrap} data-disabled={disabled || undefined}>
      {dragging && <output className={styles.readout}>{format(value)}</output>}
      <div
        className={styles.knob}
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={label}
        aria-orientation="vertical"
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={value}
        aria-valuetext={format(value)}
        aria-disabled={disabled || undefined}
        data-dragging={dragging || undefined}
        onKeyDown={disabled ? undefined : onKeyDown}
        onDoubleClick={disabled ? undefined : () => onChange(defaultValue)}
        onPointerDown={
          disabled
            ? undefined
            : (e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                drag.current = { position, pointerY: e.clientY };
                setDragging(true);
              }
        }
        onPointerMove={(e) => {
          if (!drag.current) return;
          onChange(fromPos(dragPosition(drag.current, e.clientY, e.shiftKey)));
        }}
        onPointerUp={(e) => {
          e.currentTarget.releasePointerCapture(e.pointerId);
          drag.current = null;
          setDragging(false);
        }}
      >
        <svg className={styles.ticks} viewBox="-50 -50 100 100" aria-hidden>
          {Array.from({ length: TICK_COUNT }, (_, i) => {
            const angle = (positionToAngle(i / (TICK_COUNT - 1)) * Math.PI) / 180;
            const [sin, cos] = [Math.sin(angle), Math.cos(angle)];
            return (
              <line
                key={i}
                x1={sin * 40}
                y1={-cos * 40}
                x2={sin * 47}
                y2={-cos * 47}
              />
            );
          })}
        </svg>
        <div className={styles.body}>
          <div
            className={styles.pointer}
            style={{ rotate: `${positionToAngle(position)}deg` }}
          />
          <div className={styles.cap} data-cap={cap} />
        </div>
      </div>
      <span className={styles.label}>{label}</span>
    </div>
  );
}
