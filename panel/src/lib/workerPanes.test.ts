import { describe, expect, it } from "vitest";
import { findPaneBySession, paneLeaf, paneLeaves, splitPane } from "./paneLayout";
import type { WorkerRecord, WorkerStatus } from "./protocol";
import { closeWorkerPane, showWorkerPane } from "./workerPanes";

function worker(childSessionId: string, status: WorkerStatus): WorkerRecord {
  return {
    id: `worker-${childSessionId}`, childSessionId, parentRequestId: "request",
    title: "조사", goal: "필요한 근거 조사", profile: "research", status,
    commits: [], changedFiles: [], reported: false, createdAt: 1, updatedAt: 2,
  };
}

function fullLayout() {
  const main = paneLeaf("main");
  let tree = splitPane(main, main.id, "horizontal", "first");
  tree = splitPane(tree, main.id, "vertical", "second");
  tree = splitPane(tree, main.id, "horizontal", "third");
  return { main, tree };
}

describe("automatic role worker panels", () => {
  it("closes the worker while preserving other panes and their ids", () => {
    const { tree, main } = fullLayout();
    const next = closeWorkerPane(tree, { parentSessionId: "main", childSessionId: "second" });
    expect(paneLeaves(next).map((leaf) => leaf.sessionId)).toEqual(["main", "third", "first"]);
    expect(findPaneBySession(next, "main")?.id).toBe(main.id);
  });

  it("returns a sole worker pane to its parent without deleting the last pane", () => {
    const only = paneLeaf("child");
    const next = closeWorkerPane(only, { parentSessionId: "main", childSessionId: "child" });
    expect(paneLeaves(next)).toEqual([{ ...only, sessionId: "main" }]);
  });

  it("keeps the layout when the worker pane is already closed", () => {
    const main = paneLeaf("main");
    expect(closeWorkerPane(main, { parentSessionId: "main", childSessionId: "child" })).toBe(main);
  });
  it("opens a child alongside its parent and ignores repeated creation events", () => {
    const main = paneLeaf("main");
    const request = { parentSessionId: "main", childSessionId: "research",
      focusedLeafId: main.id, workers: [] };
    const tree = showWorkerPane(main, request);
    expect(paneLeaves(tree).map((leaf) => leaf.sessionId)).toEqual(["main", "research"]);
    expect(showWorkerPane(tree, request)).toBe(tree);
  });

  it("reuses a terminal worker at capacity while preserving active work and the parent", () => {
    const { main, tree } = fullLayout();
    const next = showWorkerPane(tree, {
      parentSessionId: "main", childSessionId: "new-research", focusedLeafId: main.id,
      workers: [worker("first", { kind: "settled" }), worker("second", { kind: "running" }),
        worker("third", { kind: "queued" })],
    });
    expect(paneLeaves(next).map((leaf) => leaf.sessionId).sort())
      .toEqual(["main", "new-research", "second", "third"]);
    expect(findPaneBySession(next, "main")?.id).toBe(main.id);
  });

  it("keeps a focused terminal worker and reuses a different terminal panel", () => {
    const { tree } = fullLayout();
    const focused = findPaneBySession(tree, "first");
    const next = showWorkerPane(tree, {
      parentSessionId: "main", childSessionId: "new", focusedLeafId: focused?.id ?? null,
      workers: [worker("first", { kind: "settled" }), worker("second", { kind: "failed", reason: "error" })],
    });
    expect(findPaneBySession(next, "first")).toEqual(focused);
    expect(findPaneBySession(next, "second")).toBeUndefined();
    expect(paneLeaves(next)).toHaveLength(4);
  });

  it("keeps all panels when the only workers are active", () => {
    const { main, tree } = fullLayout();
    expect(showWorkerPane(tree, {
      parentSessionId: "main", childSessionId: "new", focusedLeafId: main.id,
      workers: [worker("first", { kind: "running" }), worker("second", { kind: "queued" })],
    })).toBe(tree);
  });

  it("opens three role workers in a balanced grid without repeatedly narrowing the parent", () => {
    const main = paneLeaf("main");
    let tree = showWorkerPane(main, { parentSessionId: "main", childSessionId: "first",
      focusedLeafId: main.id, workers: [] });
    tree = showWorkerPane(tree, { parentSessionId: "main", childSessionId: "second",
      focusedLeafId: main.id, workers: [worker("first", { kind: "running" })] });
    tree = showWorkerPane(tree, { parentSessionId: "main", childSessionId: "third",
      focusedLeafId: main.id, workers: [worker("first", { kind: "running" }), worker("second", { kind: "running" })] });
    expect(tree).toMatchObject({ kind: "split", direction: "horizontal",
      a: { kind: "split", direction: "vertical" }, b: { kind: "split", direction: "vertical" } });
  });
});
