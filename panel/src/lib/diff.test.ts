/**
 * Unified-diff line classification for the diff tab (features/03 ## Behaviors →
 * Review): "server-supplied unified diff rendered with per-line +/- coloring,
 * hunk/file-header styling".
 *
 * The diff is rendered server-side (Python difflib); the panel only classifies
 * each line for coloring — it does NOT compute the diff (rules.md: diff stays a
 * server-supplied unified diff, never Monaco DiffEditor).
 *
 * Contract (Step B implements `@/lib/diff`):
 *   export type DiffLineKind = "add" | "del" | "hunk" | "file" | "context";
 *   export interface DiffLine { kind: DiffLineKind; text: string; }
 *   export function classifyDiff(diff: string): DiffLine[];
 */
import { describe, it, expect } from "vitest";
import { classifyDiff, type DiffLine } from "@/lib/diff";

const kinds = (lines: DiffLine[]) => lines.map((l) => l.kind);

describe("classifyDiff", () => {
  it("classifies +/- lines as add/del", () => {
    const out = classifyDiff("+added\n-removed");
    expect(kinds(out)).toEqual(["add", "del"]);
  });

  it("classifies @@ hunk headers", () => {
    const out = classifyDiff("@@ -1,3 +1,4 @@");
    expect(out[0].kind).toBe("hunk");
  });

  it("classifies --- / +++ file headers as file (not del/add)", () => {
    const out = classifyDiff("--- a/x.eps\n+++ b/x.eps");
    expect(kinds(out)).toEqual(["file", "file"]);
  });

  it("classifies a leading-space / other line as context", () => {
    const out = classifyDiff(" unchanged");
    expect(out[0].kind).toBe("context");
  });

  it("preserves the raw text of each line", () => {
    const out = classifyDiff("+added line");
    expect(out[0].text).toBe("+added line");
  });

  it("returns an empty array for an empty diff", () => {
    expect(classifyDiff("")).toEqual([]);
  });
});
