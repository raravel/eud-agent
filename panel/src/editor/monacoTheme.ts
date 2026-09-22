/**
 * Monaco theme derived from the panel's own design tokens.
 *
 * index.css declares the dark palette as `oklch(...)` custom properties on
 * `.dark`; Monaco only accepts `#rrggbb[aa]`. `buildEudAgentTheme` reads each
 * token through the supplied resolver (getComputedStyle in the app, a table in
 * tests), converts it with the standard OKLab → linear sRGB → sRGB math, and
 * maps it onto Monaco's color ids and token rules. Tokens that fail to parse
 * are simply left out so the `vs-dark` base fills them; the editor never
 * receives an invalid color.
 */
import type { editor } from "monaco-editor";

export const EUD_AGENT_THEME = "eud-agent";

export type TokenResolver = (token: string) => string;

interface Oklch {
  l: number;
  c: number;
  h: number;
  alpha: number;
}

const OKLCH_PATTERN =
  /^oklch\(\s*([\d.]+%?)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+%?)\s*)?\)$/i;

function parseNumber(raw: string, percentScale: number): number {
  return raw.endsWith("%")
    ? (Number(raw.slice(0, -1)) / 100) * percentScale
    : Number(raw);
}

export function parseOklch(value: string): Oklch | null {
  const match = OKLCH_PATTERN.exec(value.trim());
  if (!match) return null;
  const l = parseNumber(match[1], 1);
  const c = Number(match[2]);
  const h = Number(match[3]);
  const alpha = match[4] === undefined ? 1 : parseNumber(match[4], 1);
  if (![l, c, h, alpha].every(Number.isFinite)) return null;
  return { l, c, h, alpha };
}

function channel(linear: number): string {
  const clamped = Math.min(1, Math.max(0, linear));
  const srgb =
    clamped <= 0.0031308
      ? 12.92 * clamped
      : 1.055 * Math.pow(clamped, 1 / 2.4) - 0.055;
  return Math.round(srgb * 255)
    .toString(16)
    .padStart(2, "0");
}

/** `oklch(L C H [/ A])` → `#rrggbb` (or `#rrggbbaa` when A < 1). */
export function oklchToHex({ l, c, h, alpha }: Oklch): string {
  const hr = (h * Math.PI) / 180;
  const a = c * Math.cos(hr);
  const b = c * Math.sin(hr);
  const l_ = l + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = l - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = l - 0.0894841775 * a - 1.291485548 * b;
  const l3 = l_ ** 3;
  const m3 = m_ ** 3;
  const s3 = s_ ** 3;
  const red = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3;
  const green = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3;
  const blue = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3;
  const rgb = `#${channel(red)}${channel(green)}${channel(blue)}`;
  if (alpha >= 1) return rgb;
  const alphaHex = Math.round(Math.min(1, Math.max(0, alpha)) * 255)
    .toString(16)
    .padStart(2, "0");
  return `${rgb}${alphaHex}`;
}

/** Resolve one `--token` to Monaco hex, optionally forcing an alpha. */
function resolve(
  read: TokenResolver,
  token: string,
  alpha?: number,
): string | undefined {
  const parsed = parseOklch(read(token));
  if (!parsed) return undefined;
  return oklchToHex(alpha === undefined ? parsed : { ...parsed, alpha });
}

/** The panel's token resolver: computed custom properties on `<html>`. */
export function documentTokenResolver(): TokenResolver {
  const style = getComputedStyle(document.documentElement);
  return (token) => style.getPropertyValue(token);
}

/**
 * Build the theme data. Surface colors follow the page (background, border,
 * ring, popover); the emerald `--primary` carries cursor, selection, keywords
 * and the scrollbar thumb at the same 32/58/78% stops index.css uses; syntax
 * colors reuse the chart accents so highlighted source reads like the rest of
 * the panel.
 */
export function buildEudAgentTheme(
  read: TokenResolver,
): editor.IStandaloneThemeData {
  const colors: Record<string, string | undefined> = {
    "editor.background": resolve(read, "--background"),
    "editor.foreground": resolve(read, "--foreground"),
    "editorGutter.background": resolve(read, "--background"),
    "editorLineNumber.foreground": resolve(read, "--muted-foreground", 0.6),
    "editorLineNumber.activeForeground": resolve(read, "--foreground"),
    "editor.lineHighlightBackground": resolve(read, "--accent", 0.5),
    "editor.lineHighlightBorder": resolve(read, "--accent", 0),
    "editorCursor.foreground": resolve(read, "--primary"),
    "editor.selectionBackground": resolve(read, "--primary", 0.3),
    "editor.inactiveSelectionBackground": resolve(read, "--primary", 0.15),
    "editor.selectionHighlightBackground": resolve(read, "--primary", 0.15),
    "editor.wordHighlightBackground": resolve(read, "--primary", 0.15),
    "editorIndentGuide.background1": resolve(read, "--border"),
    "editorIndentGuide.activeBackground1": resolve(read, "--ring"),
    "editorWhitespace.foreground": resolve(read, "--border"),
    "editorWidget.background": resolve(read, "--popover"),
    "editorWidget.border": resolve(read, "--border"),
    "editorHoverWidget.background": resolve(read, "--popover"),
    "editorHoverWidget.border": resolve(read, "--border"),
    "editorSuggestWidget.background": resolve(read, "--popover"),
    "editorSuggestWidget.border": resolve(read, "--border"),
    "editorSuggestWidget.selectedBackground": resolve(read, "--accent"),
    "focusBorder": resolve(read, "--ring"),
    "scrollbarSlider.background": resolve(read, "--primary", 0.32),
    "scrollbarSlider.hoverBackground": resolve(read, "--primary", 0.58),
    "scrollbarSlider.activeBackground": resolve(read, "--primary", 0.78),
    "scrollbar.shadow": resolve(read, "--background", 0),
    "editorBracketMatch.background": resolve(read, "--primary", 0.2),
    "editorBracketMatch.border": resolve(read, "--ring"),
    "minimap.background": resolve(read, "--background"),
  };
  const tokenColors: Array<[string, string | undefined, string?]> = [
    ["comment", resolve(read, "--muted-foreground"), "italic"],
    ["keyword", resolve(read, "--primary")],
    ["string", resolve(read, "--chart-3")],
    ["number", resolve(read, "--chart-4")],
    ["regexp", resolve(read, "--chart-5")],
    ["type.identifier", resolve(read, "--chart-2")],
    ["identifier", resolve(read, "--foreground")],
    ["delimiter", resolve(read, "--muted-foreground")],
    ["operator", resolve(read, "--muted-foreground")],
  ];
  return {
    base: "vs-dark",
    inherit: true,
    colors: Object.fromEntries(
      Object.entries(colors).filter(
        (entry): entry is [string, string] => entry[1] !== undefined,
      ),
    ),
    rules: tokenColors.flatMap(([token, foreground, fontStyle]) =>
      foreground === undefined
        ? []
        : [{ token, foreground: foreground.slice(1, 7), fontStyle }],
    ),
  };
}
