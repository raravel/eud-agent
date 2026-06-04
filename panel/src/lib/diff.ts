/**
 * Unified-diff line classification for the diff tab (features/03 ## Behaviors →
 * Review): "server-supplied unified diff rendered with per-line +/- coloring,
 * hunk/file-header styling".
 *
 * The diff is computed SERVER-SIDE (Python difflib) and shipped as a string in
 * the `code` event (rules.md: diff stays a server-supplied unified diff, NEVER
 * a Monaco DiffEditor). This module only classifies each line so the UI can
 * color it — it never computes a diff.
 */

/** Kind of a unified-diff line (drives coloring/styling). */
export type DiffLineKind = "add" | "del" | "hunk" | "file" | "context";

/** One classified diff line: its kind plus the original raw text. */
export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

/**
 * Classify each line of a unified diff. File headers (`---` / `+++`) are
 * checked BEFORE the generic `+` / `-` rules so they are not mistaken for
 * add/del lines. An empty diff yields an empty array (no rendered lines).
 */
export function classifyDiff(diff: string): DiffLine[] {
  if (diff === "") {
    return [];
  }
  return diff.split("\n").map((text) => ({ kind: classifyLine(text), text }));
}

function classifyLine(text: string): DiffLineKind {
  // File headers first — they begin with --- / +++ and must not read as del/add.
  if (text.startsWith("---") || text.startsWith("+++")) {
    return "file";
  }
  if (text.startsWith("@@")) {
    return "hunk";
  }
  if (text.startsWith("+")) {
    return "add";
  }
  if (text.startsWith("-")) {
    return "del";
  }
  return "context";
}
