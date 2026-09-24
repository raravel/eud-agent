/**
 * Pure helpers behind the project history view.
 *
 * Contract (`@/lib/gitHistory`):
 *   export function parseCommitPatch(patch: string): ParsedPatch;
 *   export function diffGutter(kind: DiffLineKind): string;
 *   export function parseCommitTrace(body: string): CommitTrace;
 *   export function shortSha(sha: string): string;
 *   export function formatCommitTime(epochSeconds: number): string;
 */
import { describe, it, expect } from "vitest";
import {
  diffGutter,
  formatCommitTime,
  parseCommitPatch,
  parseCommitTrace,
  shortSha,
} from "./gitHistory";

const patch = [
  "diff --git a/src/main.eps b/src/main.eps",
  "index 1111111..2222222 100644",
  "--- a/src/main.eps",
  "+++ b/src/main.eps",
  "@@ -1,3 +1,3 @@",
  " const keep = 1;",
  "-const old = 2;",
  "+const next = 3;",
].join("\n");

describe("parseCommitPatch", () => {
  it("drops the git preamble and keeps the hunk", () => {
    const parsed = parseCommitPatch(patch);
    expect(parsed.note).toBeNull();
    expect(parsed.lines[0]).toEqual({ kind: "hunk", text: "@@ -1,3 +1,3 @@" });
    expect(parsed.lines.map((line) => line.kind)).toEqual([
      "hunk",
      "context",
      "del",
      "add",
    ]);
    // The redundant path headers are gone: the file row already names the path.
    expect(parsed.lines.some((line) => line.text.startsWith("diff --git"))).toBe(
      false,
    );
  });

  it("notes a created file", () => {
    const created = ["diff --git a/x b/x", "new file mode 100644", "@@ -0,0 +1 @@", "+x"].join(
      "\n",
    );
    expect(parseCommitPatch(created).note).toBe("새로 만든 파일");
  });

  it("notes a deleted file", () => {
    const deleted = ["diff --git a/x b/x", "deleted file mode 100644", "@@ -1 +0,0 @@", "-x"].join(
      "\n",
    );
    expect(parseCommitPatch(deleted).note).toBe("삭제한 파일");
  });

  it("notes a rename that carries no hunk at all", () => {
    const renamed = [
      "diff --git a/old.eps b/new.eps",
      "similarity index 100%",
      "rename from old.eps",
      "rename to new.eps",
    ].join("\n");
    const parsed = parseCommitPatch(renamed);
    expect(parsed.note).toBe("이름 변경 → new.eps");
    expect(parsed.lines).toEqual([]);
  });
});

describe("diffGutter", () => {
  it("marks add and del without relying on color", () => {
    expect(diffGutter("add")).toBe("+");
    expect(diffGutter("del")).toBe("-");
    expect(diffGutter("context")).toBe(" ");
    expect(diffGutter("hunk")).toBe(" ");
  });
});

describe("parseCommitTrace", () => {
  it("reads the session and request lines", () => {
    expect(parseCommitTrace("session: s-1\nrequest: r-9\n")).toEqual({
      session: "s-1",
      request: "r-9",
    });
  });

  it("is empty for a body that carries neither", () => {
    expect(parseCommitTrace("사람이 직접 남긴 커밋")).toEqual({
      session: null,
      request: null,
    });
  });
});

describe("shortSha / formatCommitTime", () => {
  it("shortens a sha to seven characters", () => {
    expect(shortSha("a".repeat(40))).toBe("aaaaaaa");
  });

  it("reads the core's SECONDS as seconds, not milliseconds", () => {
    // The whole point of the helper: 1_700_000_000 is 2023, not 1970.
    expect(formatCommitTime(1_700_000_000)).toContain("2023-11-");
  });
});
