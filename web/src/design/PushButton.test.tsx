import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it } from 'vitest';

import { PushButton } from './PushButton';

function Mute() {
  const [pressed, setPressed] = useState(false);
  return (
    <PushButton
      label="Mute"
      ariaLabel="Mute channel 1"
      variant="mute"
      pressed={pressed}
      onToggle={setPressed}
    />
  );
}

describe('PushButton', () => {
  it('latches with aria-pressed', async () => {
    const user = userEvent.setup();
    render(<Mute />);
    const button = screen.getByRole('button', { name: 'Mute channel 1' });
    expect(button).toHaveAttribute('aria-pressed', 'false');
    await user.click(button);
    expect(button).toHaveAttribute('aria-pressed', 'true');
    await user.click(button);
    expect(button).toHaveAttribute('aria-pressed', 'false');
  });

  it('a disabled button never toggles', async () => {
    const user = userEvent.setup();
    render(
      <PushButton
        label="Arm"
        variant="arm"
        pressed={false}
        onToggle={() => {
          throw new Error('must not toggle');
        }}
        disabled
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Arm' }));
  });
});
