import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { TextField } from './TextField';

describe('TextField', () => {
  it('Enter commits the trimmed draft', async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<TextField label="Take name" value="Take 1" onCommit={onCommit} />);
    const field = screen.getByRole('textbox', { name: 'Take name' });
    await user.clear(field);
    await user.type(field, '  Take 2  {Enter}');
    expect(onCommit).toHaveBeenCalledWith('Take 2');
  });

  it('blur commits', async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<TextField label="Take name" value="Take 1" onCommit={onCommit} />);
    const field = screen.getByRole('textbox', { name: 'Take name' });
    await user.clear(field);
    await user.type(field, 'Mixdown');
    await user.tab();
    expect(onCommit).toHaveBeenCalledWith('Mixdown');
  });

  it('Escape reverts the draft without committing', async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<TextField label="Take name" value="Take 1" onCommit={onCommit} />);
    const field = screen.getByRole('textbox', { name: 'Take name' });
    await user.type(field, ' nope{Escape}');
    expect(onCommit).not.toHaveBeenCalled();
    expect(field).toHaveValue('Take 1');
  });

  it('an unchanged draft never commits', async () => {
    const user = userEvent.setup();
    const onCommit = vi.fn();
    render(<TextField label="Take name" value="Take 1" onCommit={onCommit} />);
    const field = screen.getByRole('textbox', { name: 'Take name' });
    await user.clear(field);
    await user.type(field, '  Take 1  {Enter}');
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('disabled renders a disabled field', () => {
    render(<TextField label="Take name" value="Take 1" onCommit={() => {}} disabled />);
    expect(screen.getByRole('textbox', { name: 'Take name' })).toBeDisabled();
  });
});
