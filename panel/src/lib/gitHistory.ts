/**
 * Pure helpers for the project history view.
 *
 * The core hands the panel raw `git show` output, so a file's patch still
 * carries the preamble git prints before the first hunk (`diff --git`, `index`,
 * `new file mode`, `rename to`, `--- a/…`, `+++ b/…`). The file row already
 * names the path and its +/- counts, so repeating four header lines per file
 * buries the change itself. {@link parseCommitPatch} keeps the one fact the
 * preamble carries that the row does not — created / deleted / renamed — and
 * drops the rest.
 *
 * The commit body carries `session: <id>` / `request: <id>` lines written by
 * the core's turn message. They matter for tracing a change back to a
 * conversation, not for reading, so {@link parseCommitTrace} pulls them out and
 * the view renders them quietly.
 */
import { classifyDiff, type DiffLine } from "@/lib/diff";

/** A file patch split into its one-line note and its rendered diff lines. */
export interface ParsedPatch {
  /** Korean note for a created/deleted/renamed file, or null. */
  note: string | null;
  /** The diff from its first hunk onward (the whole patch when it has none). */
  lines: DiffLine[];
}

/** Split a raw `git show` file patch into a note plus its hunk lines. */
export function parseCommitPatch(patch: string): ParsedPatch {
  const all = patch.split("\n");
  const firstHunk = all.findIndex((line) => line.startsWith("@@"));
  const preamble = firstHunk < 0 ? all : all.slice(0, firstHunk);
  let note: string | null = null;
  for (const line of preamble) {
    if (line.startsWith("new file mode")) note = "새로 만든 파일";
    else if (line.startsWith("deleted file mode")) note = "삭제한 파일";
    else if (line.startsWith("rename to ")) {
      note = `이름 변경 → ${line.slice("rename to ".length).trim()}`;
    }
  }
  // No hunk at all (a pure rename or mode change): there is nothing to render
  // as a diff, and the note alone tells the whole story.
  const body = firstHunk < 0 ? "" : all.slice(firstHunk).join("\n");
  return { note, lines: classifyDiff(body) };
}

/** The gutter marker a diff line shows next to its text (never color alone). */
export function diffGutter(kind: DiffLine["kind"]): string {
  if (kind === "add") return "+";
  if (kind === "del") return "-";
  return " ";
}

/** Where one commit came from, as its body records it. */
export interface CommitTrace {
  session: string | null;
  request: string | null;
}

/** Read the `session:` / `request:` trace lines out of a commit body. */
export function parseCommitTrace(body: string): CommitTrace {
  const trace: CommitTrace = { session: null, request: null };
  for (const raw of body.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("session:")) {
      trace.session = line.slice("session:".length).trim() || null;
    } else if (line.startsWith("request:")) {
      trace.request = line.slice("request:".length).trim() || null;
    }
  }
  return trace;
}

/** The short sha the history rows show. */
export function shortSha(sha: string): string {
  return sha.slice(0, 7);
}

/**
 * A commit timestamp as the history shows it. The core reports SECONDS since
 * the epoch; `Date` wants milliseconds, and the ×1000 is the whole bug class
 * this helper exists to keep out of the view.
 */
export function formatCommitTime(epochSeconds: number): string {
  const date = new Date(epochSeconds * 1000);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate(),
  )} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
