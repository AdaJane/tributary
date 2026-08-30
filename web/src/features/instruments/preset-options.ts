/**
 * Turning a SoundFont's preset list into a picker.
 *
 * Pure, and separate from the component, because the interesting part is
 * the addressing: a preset is a (bank, program) pair, a `<select>` value is
 * one string, and getting that round trip wrong silently loads the wrong
 * sound. The value is `"<bank>:<program>"` and nothing else parses it.
 */
import type { SelectOption } from '../../design/select-options';

export interface Preset {
  bank: number;
  program: number;
  name: string;
}

/** The one spelling of a preset's identity in this file's world. */
export function presetValue(bank: number, program: number): string {
  return `${bank}:${program}`;
}

/**
 * `null` for anything that is not a value this module produced — a caller
 * that guessed at the format gets nothing rather than bank 0 program 0,
 * which would be a plausible-looking wrong answer.
 */
export function parsePresetValue(value: string): { bank: number; program: number } | null {
  const match = /^(\d+):(\d+)$/.exec(value);
  if (!match) return null;
  const bank = Number(match[1]);
  const program = Number(match[2]);
  if (bank > 16_383 || program > 127) return null;
  return { bank, program };
}

/**
 * Options for the picker, in the order a player scrolls: General MIDI
 * order, bank then program. The program number stays visible because it is
 * what a MIDI file and a hardware controller both speak — a name alone
 * would make "program 40" unanswerable.
 */
export function presetOptions(presets: readonly Preset[]): SelectOption[] {
  return [...presets]
    .sort((a, b) => a.bank - b.bank || a.program - b.program)
    .map((p) => ({
      value: presetValue(p.bank, p.program),
      // Bank shown only when it is not the General MIDI one, so the common
      // case reads as "040 Violin" rather than as a coordinate pair.
      label:
        p.bank === 0
          ? `${String(p.program).padStart(3, '0')}  ${p.name}`
          : `${p.bank}:${String(p.program).padStart(3, '0')}  ${p.name}`,
    }));
}

/**
 * What the picker shows while the list is still loading, or when the file
 * could not be read: the numbers the instrument actually holds. Never an
 * empty box — the instrument does have a preset, we just cannot name it.
 */
export function presetFallback(bank: number, program: number): SelectOption[] {
  return [
    {
      value: presetValue(bank, program),
      label: `${bank}:${String(program).padStart(3, '0')}`,
    },
  ];
}
