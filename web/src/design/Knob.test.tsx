import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { formatDb } from '../audio/db';
import { Knob } from './Knob';

function GainKnob({ onChange = () => {} }: { onChange?: (v: number) => void }) {
  const [value, setValue] = useState(0);
  return (
    <Knob
      label="Gain"
      value={value}
      min={-20}
      max={60}
      defaultValue={0}
      step={0.5}
      bigStep={5}
      format={formatDb}
      cap="red"
      onChange={(v) => {
        setValue(v);
        onChange(v);
      }}
    />
  );
}

describe('Knob', () => {
  it('exposes the slider contract with a human-readable value', () => {
    render(<GainKnob />);
    const knob = screen.getByRole('slider', { name: 'Gain' });
    expect(knob).toHaveAttribute('aria-valuemin', '-20');
    expect(knob).toHaveAttribute('aria-valuemax', '60');
    expect(knob).toHaveAttribute('aria-valuetext', '0.0');
  });

  it('arrow keys step, PageUp big-steps, Home/End clamp', async () => {
    const user = userEvent.setup();
    render(<GainKnob />);
    const knob = screen.getByRole('slider', { name: 'Gain' });
    knob.focus();
    await user.keyboard('{ArrowUp}');
    expect(knob).toHaveAttribute('aria-valuenow', '0.5');
    await user.keyboard('{PageUp}');
    expect(knob).toHaveAttribute('aria-valuenow', '5.5');
    await user.keyboard('{End}');
    expect(knob).toHaveAttribute('aria-valuenow', '60');
    await user.keyboard('{Home}');
    expect(knob).toHaveAttribute('aria-valuenow', '-20');
  });

  it('double-click resets to the default value', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<GainKnob onChange={onChange} />);
    const knob = screen.getByRole('slider', { name: 'Gain' });
    knob.focus();
    await user.keyboard('{PageUp}');
    await user.dblClick(knob);
    expect(onChange).toHaveBeenLastCalledWith(0);
  });

  it('a disabled knob leaves the tab order and ignores keys', async () => {
    const user = userEvent.setup();
    render(
      <Knob
        label="Pan"
        value={0}
        min={-1}
        max={1}
        defaultValue={0}
        format={String}
        cap="white"
        onChange={() => {}}
        disabled
      />,
    );
    const knob = screen.getByRole('slider', { name: 'Pan' });
    expect(knob).toHaveAttribute('tabindex', '-1');
    expect(knob).toHaveAttribute('aria-disabled', 'true');
    knob.focus();
    await user.keyboard('{ArrowUp}');
    expect(knob).toHaveAttribute('aria-valuenow', '0');
  });
});
