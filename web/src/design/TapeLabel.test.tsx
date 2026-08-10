import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { TapeLabel } from './TapeLabel';

describe('TapeLabel', () => {
  it('double-click edits, Enter commits the trimmed name', async () => {
    const user = userEvent.setup();
    const onRename = vi.fn();
    render(<TapeLabel id="strip-1" name="Kick" onRename={onRename} />);
    await user.dblClick(
      screen.getByRole('button', { name: /Rename channel, currently “Kick”/ }),
    );
    const input = screen.getByRole('textbox', { name: 'Channel name' });
    await user.clear(input);
    await user.type(input, '  Snare  {Enter}');
    expect(onRename).toHaveBeenCalledWith('Snare');
  });

  it('Escape reverts without renaming and focus returns to the tape', async () => {
    const user = userEvent.setup();
    const onRename = vi.fn();
    render(<TapeLabel id="strip-1" name="Kick" onRename={onRename} />);
    const tape = screen.getByRole('button', { name: /Kick/ });
    await user.dblClick(tape);
    await user.keyboard('nope{Escape}');
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: /Kick/ })).toHaveFocus();
  });

  it('an empty draft is discarded, not committed', async () => {
    const user = userEvent.setup();
    const onRename = vi.fn();
    render(<TapeLabel id="strip-1" name="Kick" onRename={onRename} />);
    await user.dblClick(screen.getByRole('button', { name: /Kick/ }));
    await user.clear(screen.getByRole('textbox'));
    await user.keyboard('{Enter}');
    expect(onRename).not.toHaveBeenCalled();
  });

  it('without onRename the tape is plain and not editable', async () => {
    const user = userEvent.setup();
    render(<TapeLabel id="p" name="Friday Night" />);
    const tape = screen.getByRole('button', { name: 'Channel Friday Night' });
    await user.dblClick(tape);
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  });
});
