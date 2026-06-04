/**
 * Preview-truncation helper (features/03 ## Behaviors → Review):
 *   "display truncated at 1 MiB measured AND sliced in the same UTF-16
 *    code-unit metric (vanilla advisory fix) with a notice; Apply always sends
 *    full text."
 *
 * The vanilla panel measured the limit in UTF-8 BYTES but sliced in UTF-16 code
 * units, so a string just over 1 MiB of multi-byte text was cut at the wrong
 * index. This suite pins the consistent metric: BOTH the measure (`.length`)
 * and the slice (`.slice`) use UTF-16 code units, and the boundary is exact.
 *
 * Contract (Step B implements `@/lib/truncate`):
 *   export const PREVIEW_LIMIT = 1024 * 1024;            // 1 MiB in code units
 *   export interface Truncated { text: string; truncated: boolean; }
 *   export function truncateForDisplay(code: string, limit?: number): Truncated;
 */
import { describe, it, expect } from "vitest";
import {
  PREVIEW_LIMIT,
  truncateForDisplay,
} from "@/lib/truncate";

describe("PREVIEW_LIMIT", () => {
  it("is exactly 1 MiB (1024 * 1024) in UTF-16 code units", () => {
    expect(PREVIEW_LIMIT).toBe(1024 * 1024);
  });
});

describe("truncateForDisplay — UTF-16 code-unit consistency", () => {
  it("leaves a string at the exact limit untouched (boundary, not truncated)", () => {
    const code = "a".repeat(PREVIEW_LIMIT);
    const out = truncateForDisplay(code);
    expect(out.truncated).toBe(false);
    expect(out.text.length).toBe(PREVIEW_LIMIT);
    expect(out.text).toBe(code);
  });

  it("truncates a string one code unit over the limit to exactly the limit", () => {
    const code = "a".repeat(PREVIEW_LIMIT + 1);
    const out = truncateForDisplay(code);
    expect(out.truncated).toBe(true);
    // Sliced in UTF-16 code units → length is exactly the limit.
    expect(out.text.length).toBe(PREVIEW_LIMIT);
  });

  it("measures and slices in the SAME UTF-16 metric for multi-byte text (the vanilla bug)", () => {
    // "가" is 1 UTF-16 code unit but 3 UTF-8 bytes. A byte-based measure would
    // wrongly flag this (3 MiB of bytes) as over-limit; the code-unit measure
    // does not. Use a small custom limit to make the assertion crisp.
    const limit = 4;
    const code = "가가가가"; // 4 code units, 12 UTF-8 bytes
    const out = truncateForDisplay(code, limit);
    expect(out.truncated).toBe(false);
    expect(out.text).toBe(code);
    expect(out.text.length).toBe(4);
  });

  it("never splits using a byte metric: over-limit multi-byte text cut at code-unit index", () => {
    const limit = 3;
    const code = "가나다라마"; // 5 code units
    const out = truncateForDisplay(code, limit);
    expect(out.truncated).toBe(true);
    expect(out.text.length).toBe(3);
    expect(out.text).toBe("가나다");
  });

  it("does not flag a short string", () => {
    const out = truncateForDisplay("hello", PREVIEW_LIMIT);
    expect(out.truncated).toBe(false);
    expect(out.text).toBe("hello");
  });
});
