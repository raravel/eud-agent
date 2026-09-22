import { describe, expect, it } from "vitest";

import {
  EUD_AGENT_THEME,
  buildEudAgentTheme,
  oklchToHex,
  parseOklch,
} from "./monacoTheme";

// The `.dark` palette from index.css, as getComputedStyle reports it.
const DARK_TOKENS: Record<string, string> = {
  "--background": "oklch(0.14 0.012 255)",
  "--foreground": "oklch(0.97 0.005 250)",
  "--popover": "oklch(0.18 0.012 255)",
  "--primary": "oklch(0.7 0.15 162)",
  "--muted-foreground": "oklch(0.72 0.012 250)",
  "--accent": "oklch(0.23 0.012 255)",
  "--border": "oklch(1 0 0 / 10%)",
  "--ring": "oklch(0.6 0.12 162)",
  "--chart-2": "oklch(0.696 0.17 162.48)",
  "--chart-3": "oklch(0.769 0.188 70.08)",
  "--chart-4": "oklch(0.627 0.265 303.9)",
  "--chart-5": "oklch(0.645 0.246 16.439)",
};

describe("oklch → Monaco hex", () => {
  it("parses the custom-property syntax index.css uses", () => {
    expect(parseOklch(" oklch(0.14 0.012 255) ")).toEqual({
      l: 0.14,
      c: 0.012,
      h: 255,
      alpha: 1,
    });
    expect(parseOklch("oklch(1 0 0 / 10%)")).toEqual({ l: 1, c: 0, h: 0, alpha: 0.1 });
    expect(parseOklch("oklch(50% 0.1 20 / 0.5)")).toEqual({
      l: 0.5,
      c: 0.1,
      h: 20,
      alpha: 0.5,
    });
    expect(parseOklch("#ffffff")).toBeNull();
    expect(parseOklch("color-mix(in oklch, var(--muted) 42%, transparent)")).toBeNull();
    expect(parseOklch("")).toBeNull();
  });

  it("converts achromatic anchors exactly and keeps alpha as a suffix", () => {
    expect(oklchToHex({ l: 1, c: 0, h: 0, alpha: 1 })).toBe("#ffffff");
    expect(oklchToHex({ l: 0, c: 0, h: 0, alpha: 1 })).toBe("#000000");
    expect(oklchToHex({ l: 1, c: 0, h: 0, alpha: 0.1 })).toBe("#ffffff1a");
  });

  it("lands the emerald primary in the green sRGB region", () => {
    const hex = oklchToHex(parseOklch(DARK_TOKENS["--primary"])!);
    const [r, g, b] = [1, 3, 5].map((offset) =>
      Number.parseInt(hex.slice(offset, offset + 2), 16),
    );
    expect(g).toBeGreaterThan(r + 60);
    expect(g).toBeGreaterThan(b + 30);
  });
});

describe("buildEudAgentTheme", () => {
  it("maps page tokens onto Monaco surface colors and syntax rules", () => {
    const theme = buildEudAgentTheme((token) => DARK_TOKENS[token] ?? "");
    expect(EUD_AGENT_THEME).toBe("eud-agent");
    expect(theme.base).toBe("vs-dark");
    expect(theme.inherit).toBe(true);
    const background = oklchToHex(parseOklch(DARK_TOKENS["--background"])!);
    const primary = oklchToHex(parseOklch(DARK_TOKENS["--primary"])!);
    expect(theme.colors["editor.background"]).toBe(background);
    expect(theme.colors["editorGutter.background"]).toBe(background);
    expect(theme.colors["editorCursor.foreground"]).toBe(primary);
    // Alpha overrides follow the scrollbar stops index.css declares.
    expect(theme.colors["scrollbarSlider.background"]).toBe(`${primary}52`);
    expect(theme.colors["scrollbarSlider.hoverBackground"]).toBe(`${primary}94`);
    expect(theme.colors["editorIndentGuide.background1"]).toBe("#ffffff1a");
    for (const value of Object.values(theme.colors)) {
      expect(value).toMatch(/^#[0-9a-f]{6}([0-9a-f]{2})?$/);
    }
    const keyword = theme.rules.find((rule) => rule.token === "keyword");
    expect(keyword?.foreground).toBe(primary.slice(1));
    const comment = theme.rules.find((rule) => rule.token === "comment");
    expect(comment?.fontStyle).toBe("italic");
    for (const rule of theme.rules) {
      expect(rule.foreground).toMatch(/^[0-9a-f]{6}$/);
    }
  });

  it("leaves unresolvable tokens to the base theme instead of emitting junk", () => {
    const theme = buildEudAgentTheme(() => "");
    expect(theme.colors).toEqual({});
    expect(theme.rules).toEqual([]);
  });
});
