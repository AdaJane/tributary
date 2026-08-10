/**
 * The design-system boundary, enforced: components express color and type
 * only through tokens. Raw hex, rgb()/hsl()/oklch(), and literal font
 * families belong in design/tokens.css alone — one fact, one home.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { describe, expect, it } from 'vitest';

const SRC_ROOT = join(import.meta.dirname, '..');

/** Files where raw color/type facts are allowed to live. */
const EXEMPT = new Set(['design/tokens.css', 'design/fonts.css']);

const RAW_HEX = /#[0-9a-fA-F]{3,8}\b/;
const RAW_COLOR_FN = /\b(?:rgb|hsl|oklch)a?\(/;
/** font-family (CSS) or fontFamily (JSX style) not immediately fed a token.
 * Whitespace lives inside the lookahead — outside it, `\s*` backtracks one
 * space and the guard never fires. */
const RAW_FONT = /font-?[Ff]amily\s*:(?!\s*['"]?var\()/;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      return entry === 'generated' ? [] : sourceFiles(path);
    }
    const scannable = /\.(css|tsx|ts)$/.test(entry) && !/\.test\./.test(entry);
    return scannable ? [path] : [];
  });
}

describe('token guard', () => {
  const offenders = (pattern: RegExp) =>
    sourceFiles(SRC_ROOT)
      .map((path) => relative(SRC_ROOT, path).split(sep).join('/'))
      .filter((rel) => !EXEMPT.has(rel))
      .filter((rel) =>
        pattern.test(readFileSync(join(SRC_ROOT, rel), 'utf8')),
      );

  it('no raw hex colors outside tokens.css', () => {
    expect(offenders(RAW_HEX)).toEqual([]);
  });

  it('no rgb()/hsl()/oklch() outside tokens.css', () => {
    expect(offenders(RAW_COLOR_FN)).toEqual([]);
  });

  it('no literal font families outside tokens.css', () => {
    expect(offenders(RAW_FONT)).toEqual([]);
  });
});
