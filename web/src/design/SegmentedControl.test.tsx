import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { SegmentedControl } from './SegmentedControl';

const FORMATS = [
  { value: 'wav', label: 'WAV' },
  { value: 'flac', label: 'FLAC' },
  { value: 'mp3', label: 'MP3', ariaLabel: 'MP3 (lossy)' },
] as const;

type Format = (typeof FORMATS)[number]['value'];

function FormatPicker({ onChange }: { onChange?: (value: Format) => void }) {
  const [value, setValue] = useState<Format>('wav');
  return (
    <SegmentedControl
      label="File format"
      options={FORMATS}
      value={value}
      onChange={(next) => {
        setValue(next);
        onChange?.(next);
      }}
    />
  );
}

describe('SegmentedControl', () => {
  it('renders a labelled radiogroup with the checked option', () => {
    render(<FormatPicker />);
    const group = screen.getByRole('radiogroup', { name: 'File format' });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: 'WAV' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'FLAC' })).not.toBeChecked();
    expect(screen.getByRole('radio', { name: 'MP3 (lossy)' })).not.toBeChecked();
  });

  it('click selects and reports the value', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<FormatPicker onChange={onChange} />);
    await user.click(screen.getByRole('radio', { name: 'FLAC' }));
    expect(onChange).toHaveBeenCalledWith('flac');
    expect(screen.getByRole('radio', { name: 'FLAC' })).toBeChecked();
  });

  it('ArrowRight moves selection and focus, wrapping at the end', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<FormatPicker onChange={onChange} />);
    screen.getByRole('radio', { name: 'WAV' }).focus();
    await user.keyboard('{ArrowRight}');
    expect(onChange).toHaveBeenLastCalledWith('flac');
    expect(screen.getByRole('radio', { name: 'FLAC' })).toHaveFocus();
    await user.keyboard('{ArrowRight}{ArrowRight}');
    expect(onChange).toHaveBeenLastCalledWith('wav');
    expect(screen.getByRole('radio', { name: 'WAV' })).toHaveFocus();
    expect(screen.getByRole('radio', { name: 'WAV' })).toBeChecked();
  });

  it('Home and End jump to the first and last option', async () => {
    const user = userEvent.setup();
    render(<FormatPicker />);
    screen.getByRole('radio', { name: 'WAV' }).focus();
    await user.keyboard('{End}');
    expect(screen.getByRole('radio', { name: 'MP3 (lossy)' })).toBeChecked();
    expect(screen.getByRole('radio', { name: 'MP3 (lossy)' })).toHaveFocus();
    await user.keyboard('{Home}');
    expect(screen.getByRole('radio', { name: 'WAV' })).toBeChecked();
  });

  it('only the checked option is tabbable', async () => {
    const user = userEvent.setup();
    render(<FormatPicker />);
    expect(screen.getByRole('radio', { name: 'WAV' })).toHaveAttribute('tabindex', '0');
    expect(screen.getByRole('radio', { name: 'FLAC' })).toHaveAttribute('tabindex', '-1');
    await user.click(screen.getByRole('radio', { name: 'FLAC' }));
    expect(screen.getByRole('radio', { name: 'WAV' })).toHaveAttribute('tabindex', '-1');
    expect(screen.getByRole('radio', { name: 'FLAC' })).toHaveAttribute('tabindex', '0');
  });

  it('a disabled group never changes', async () => {
    const user = userEvent.setup();
    render(
      <SegmentedControl
        label="File format"
        options={FORMATS}
        value="wav"
        onChange={() => {
          throw new Error('must not change');
        }}
        disabled
      />,
    );
    await user.click(screen.getByRole('radio', { name: 'FLAC' }));
  });
});
