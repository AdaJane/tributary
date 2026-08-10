import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';

import { FADER_MIN_DB } from '../audio/db';
import { Fader } from './Fader';

function ChannelFader() {
  const [value, setValue] = useState(FADER_MIN_DB);
  return <Fader label="Channel 1 fader" value={value} onChange={setValue} cap="white" />;
}

describe('Fader', () => {
  it('starts at the floor and reads −∞', () => {
    render(<ChannelFader />);
    const fader = screen.getByRole('slider', { name: 'Channel 1 fader' });
    expect(fader).toHaveAttribute('aria-valuetext', '-∞');
  });

  it('keyboard steps in dB and double-click returns to unity', async () => {
    const user = userEvent.setup();
    render(<ChannelFader />);
    const fader = screen.getByRole('slider', { name: 'Channel 1 fader' });
    fader.focus();
    await user.keyboard('{ArrowUp}');
    expect(fader).toHaveAttribute('aria-valuenow', '-89.5');
    await user.dblClick(fader);
    expect(fader).toHaveAttribute('aria-valuenow', '0');
    expect(fader).toHaveAttribute('aria-valuetext', '0.0');
    await user.keyboard('{PageDown}');
    expect(fader).toHaveAttribute('aria-valuenow', '-3');
  });

  it('Home jumps to max, End to the floor', async () => {
    const user = userEvent.setup();
    render(<ChannelFader />);
    const fader = screen.getByRole('slider', { name: 'Channel 1 fader' });
    fader.focus();
    await user.keyboard('{Home}');
    expect(fader).toHaveAttribute('aria-valuenow', '10');
    await user.keyboard('{End}');
    expect(fader).toHaveAttribute('aria-valuetext', '-∞');
  });
});
