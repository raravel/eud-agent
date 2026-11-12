import { describe, expect, it } from "vitest";

import { languageFromLocale } from "@/lib/i18n";

import en from "./en.json";
import ko from "./ko.json";

type Tree = { [key: string]: string | Tree };

function leaves(tree: Tree, prefix = ""): Map<string, string> {
  const out = new Map<string, string>();
  for (const [key, value] of Object.entries(tree)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.set(path, value);
    else for (const [k, v] of leaves(value, path)) out.set(k, v);
  }
  return out;
}

const placeholders = (text: string) => [...text.matchAll(/\{\{\s*(\w+)\s*\}\}/g)].map((m) => m[1]).sort();

describe("locales", () => {
  const koLeaves = leaves(ko as Tree);
  const enLeaves = leaves(en as Tree);

  it("carry the same keys in Korean and English", () => {
    const missingInEn = [...koLeaves.keys()].filter((key) => !enLeaves.has(key));
    const missingInKo = [...enLeaves.keys()].filter((key) => !koLeaves.has(key));
    expect({ missingInEn, missingInKo }).toEqual({ missingInEn: [], missingInKo: [] });
  });

  it("use the same placeholders in both languages", () => {
    const mismatched = [...koLeaves].filter(
      ([key, text]) => enLeaves.has(key) && placeholders(text).join() !== placeholders(enLeaves.get(key)!).join(),
    );
    expect(mismatched.map(([key]) => key)).toEqual([]);
  });

  it("leave no Hangul in English text except the Korean language name", () => {
    const hangul = /[가-힣]/;
    const offenders = [...enLeaves]
      .filter(([key, text]) => hangul.test(text) && key !== "settings.language.ko")
      .map(([key]) => key);
    expect(offenders).toEqual([]);
  });
});

describe("languageFromLocale", () => {
  it("is Korean only for a Korean locale", () => {
    for (const locale of ["ko", "ko-KR", "ko_KR", "KO"]) expect(languageFromLocale(locale)).toBe("ko");
    for (const locale of ["en-US", "ja", "", undefined, "kok"]) expect(languageFromLocale(locale)).toBe("en");
  });
});
