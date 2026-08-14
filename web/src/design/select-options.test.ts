import { describe, expect, it } from 'vitest';

import { withCurrentValue } from './select-options';

describe('withCurrentValue', () => {
  const ports = [
    { value: 'nanoKEY2', label: 'nanoKEY2' },
    { value: 'Midi Through', label: 'Midi Through' },
  ];

  it('a value the list does not contain is appended rather than rendered blank', () => {
    // The bug this exists for: a `<select>` whose value matches no option
    // silently displays the FIRST one, so an unplugged port would read as
    // whichever port happens to sort first — a confident lie.
    const options = withCurrentValue(ports, 'Scarlett MIDI');
    expect(options).toHaveLength(3);
    expect(options[2]).toEqual({
      value: 'Scarlett MIDI',
      label: 'Scarlett MIDI (not connected)',
    });
  });

  it('leaves a list that already holds the value alone', () => {
    expect(withCurrentValue(ports, 'nanoKEY2')).toEqual(ports);
  });

  it('treats the empty value as "nothing chosen", not as a missing option', () => {
    expect(withCurrentValue(ports, '')).toEqual(ports);
  });
});
