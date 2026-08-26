import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

/**
 * Contrast verification for the design-token palettes (Phase 3.5).
 * Parses src/theme/tokens.css so CI fails if any future token edit drops a
 * text-bearing pair below WCAG 2.x AA (>= 4.5:1) in either theme.
 */

interface Palette {
  [token: string]: string;
}

function parseBlock(css: string, selector: RegExp): Palette {
  const match = css.match(selector);
  if (match === null || match[1] === undefined) {
    throw new Error('tokens.css block not found');
  }
  const palette: Palette = {};
  const declarations = match[1].matchAll(/(--color[\w-]*)\s*:\s*([^;]+);/g);
  for (const declaration of declarations) {
    const name = declaration[1];
    const value = declaration[2];
    if (name === undefined || value === undefined) {
      continue;
    }
    palette[name] = value.trim();
  }
  return palette;
}

const CSS_PATH = join(import.meta.dirname, 'tokens.css');
const css = readFileSync(CSS_PATH, 'utf8');
const light = parseBlock(css, /:root\s*\{([^}]*)\}/);
const dark = parseBlock(css, /\[data-theme='dark'\]\s*\{([^}]*)\}/);

type Rgb = readonly [number, number, number];

function hexToRgb(hex: string): Rgb {
  const raw = hex.replace('#', '');
  const full =
    raw.length === 3
      ? raw
          .split('')
          .map((c) => c + c)
          .join('')
      : raw;
  const value = Number.parseInt(full, 16);
  return [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff];
}

/** Composites an alpha color over an opaque base color. */
function composite(top: string, base: Rgb): Rgb {
  const alphaMatch = top.match(/rgba?\(([^)]+)\)/);
  if (alphaMatch === null) {
    return hexToRgb(top);
  }
  const parts = alphaMatch[1]!.split(',').map((piece) => Number.parseFloat(piece.trim()));
  const [r, g, b, alpha = 1] = parts;
  const blend = (topChannel: number | undefined, baseChannel: number): number =>
    Math.round((topChannel ?? 0) * alpha + baseChannel * (1 - alpha));
  return [blend(r, base[0]), blend(g, base[1]), blend(b, base[2])];
}

function luminance([r, g, b]: Rgb): number {
  const channel = (value: number): number => {
    const scaled = value / 255;
    return scaled <= 0.03928 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrast(a: Rgb, b: Rgb): number {
  const la = luminance(a);
  const lb = luminance(b);
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * Resolves a foreground/background pair exactly as the CSS stack renders
 * it: alpha tokens composite over their real backdrop.
 */
function resolvePair(palette: Palette, fgToken: string, bgToken: string): { fg: Rgb; bg: Rgb } {
  const fgValue = palette[fgToken];
  const bgValue = palette[bgToken];
  const surface = palette['--color-surface'];
  if (fgValue === undefined || bgValue === undefined || surface === undefined) {
    throw new Error(`missing token ${fgToken} / ${bgToken} / --color-surface`);
  }
  // Alpha backgrounds sit on the surface color in every current usage.
  const bg = bgValue.startsWith('rgba') ? composite(bgValue, hexToRgb(surface)) : hexToRgb(bgValue);
  const fg = fgValue.startsWith('rgba') ? composite(fgValue, bg) : hexToRgb(fgValue);
  return { fg, bg };
}

const TEXT_PAIRS: Array<{ fg: string; bg: string }> = [
  { fg: '--color-text', bg: '--color-bg' },
  { fg: '--color-text', bg: '--color-surface' },
  { fg: '--color-text-muted', bg: '--color-bg' },
  { fg: '--color-text-muted', bg: '--color-surface' },
  { fg: '--color-accent', bg: '--color-bg' },
  { fg: '--color-accent', bg: '--color-surface' },
];

// Colored text rendered on its translucent tint over the card surface:
// active tab stops, palette selection rows, and status pills.
const TINT_PAIRS: Array<{ fg: string; tint: string }> = [
  { fg: '--color-accent', tint: '--color-accent-soft' },
  { fg: '--color-success', tint: '--color-success-soft' },
  { fg: '--color-warning', tint: '--color-warning-soft' },
  { fg: '--color-danger', tint: '--color-danger-soft' },
];

const BUTTON_PAIRS: Array<{ fg: string; bg: string }> = [
  { fg: '--color-accent-contrast', bg: '--color-accent' },
  { fg: '--color-accent-contrast', bg: '--color-accent-hover' },
];

describe.each([
  ['light', light],
  ['dark', dark],
])('%s theme token contrast', (_name, palette) => {
  const AA = 4.5;

  it.each(TEXT_PAIRS)('$fg on $bg meets AA text contrast', ({ fg, bg }) => {
    const { fg: front, bg: back } = resolvePair(palette, fg, bg);
    expect(contrast(front, back)).toBeGreaterThanOrEqual(AA);
  });

  it.each(TINT_PAIRS)('$fg on $tint over surface meets AA text contrast', ({ fg, tint }) => {
    const { fg: front, bg: back } = resolvePair(palette, fg, tint);
    expect(contrast(front, back)).toBeGreaterThanOrEqual(AA);
  });

  it.each(BUTTON_PAIRS)('$fg on $bg meets AA text contrast', ({ fg, bg }) => {
    const { fg: front, bg: back } = resolvePair(palette, fg, bg);
    expect(contrast(front, back)).toBeGreaterThanOrEqual(AA);
  });

  it('keeps muted text at least half the strength of body text', () => {
    // Guards against "fixing" contrast by making everything identical.
    expect(luminance(hexToRgb(palette['--color-text-muted']!))).not.toBe(
      luminance(hexToRgb(palette['--color-text']!)),
    );
  });
});
