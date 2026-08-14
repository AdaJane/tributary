/**
 * Option-list rules for SelectField. Pure — the component renders, this
 * decides, and the deciding is the part with a bug in it.
 */

export interface SelectOption {
  value: string;
  label: string;
}

/**
 * The current value, always present in the list.
 *
 * MASTER.md: "A value that is not in the list renders **blank and lies**."
 * A `<select>` whose `value` matches no `<option>` shows the first one — so
 * a patch pointing at a device that has gone away would silently read as
 * pointing at whatever happens to be first. Appending it, marked, is the
 * difference between "your MIDI port is unplugged" and a lie.
 */
export function withCurrentValue(
  options: readonly SelectOption[],
  value: string,
  suffix = 'not connected',
): SelectOption[] {
  if (value === '' || options.some((option) => option.value === value)) {
    return [...options];
  }
  return [...options, { value, label: `${value} (${suffix})` }];
}
