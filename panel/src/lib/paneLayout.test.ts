import { describe, expect, it } from "vitest";

import {
  MAX_PANES,
  closePane,
  focusNeighbor,
  loadPaneLayout,
  paneLeaf,
  paneLeaves,
  paneSibling,
  paneStorageKey,
  parsePaneLayout,
  prunePanes,
  replacePaneSession,
  savePaneLayout,
  serializePaneLayout,
  showSession,
  splitPane,
  type PaneNode,
} from "./paneLayout";

function sessions(tree: PaneNode): string[] {
  return paneLeaves(tree).map((leaf) => leaf.sessionId);
}

function leafOf(tree: PaneNode, sessionId: string): string {
  const leaf = paneLeaves(tree).find((candidate) => candidate.sessionId === sessionId);
  if (!leaf) throw new Error(`no pane shows ${sessionId}`);
  return leaf.id;
}

/** a | b over c, d beside c: [a | b] / [c | d]. */
function grid(): PaneNode {
  let tree: PaneNode = paneLeaf("a");
  tree = splitPane(tree, leafOf(tree, "a"), "vertical", "c");
  tree = splitPane(tree, leafOf(tree, "a"), "horizontal", "b");
  tree = splitPane(tree, leafOf(tree, "c"), "horizontal", "d");
  return tree;
}

describe("paneLayout", () => {
  it("splits a pane, keeping the existing session first", () => {
    const root = paneLeaf("a");
    const tree = splitPane(root, root.id, "horizontal", "b");
    expect(tree.kind).toBe("split");
    if (tree.kind !== "split") return;
    expect(tree.direction).toBe("horizontal");
    expect(sessions(tree)).toEqual(["a", "b"]);
    expect(tree.a).toBe(root);
  });

  it("refuses a fifth pane and a session already shown", () => {
    const tree = grid();
    expect(paneLeaves(tree)).toHaveLength(MAX_PANES);
    expect(splitPane(tree, leafOf(tree, "a"), "horizontal", "e")).toBe(tree);
    const root = paneLeaf("a");
    const pair = splitPane(root, root.id, "vertical", "b");
    expect(splitPane(pair, leafOf(pair, "a"), "vertical", "b")).toBe(pair);
    expect(sessions(splitPane(root, "missing", "vertical", "b"))).toEqual(["a"]);
  });

  it("closes a pane into its sibling and never closes the last pane", () => {
    const tree = grid();
    const closed = closePane(tree, leafOf(tree, "b"));
    expect(sessions(closed)).toEqual(["a", "c", "d"]);
    if (closed.kind !== "split") throw new Error("expected a split");
    expect(closed.a.kind).toBe("leaf");
    // Focus moves to the pane that takes the closed pane's place.
    expect(paneSibling(tree, leafOf(tree, "d"))?.sessionId).toBe("c");
    expect(paneSibling(tree, leafOf(tree, "a"))?.sessionId).toBe("b");
    const single = paneLeaf("only");
    expect(closePane(single, single.id)).toBe(single);
    const again = closePane(closePane(closed, leafOf(closed, "c")), leafOf(closed, "d"));
    expect(sessions(again)).toEqual(["a"]);
  });

  it("finds the geometric neighbour in every direction", () => {
    const tree = grid();
    const at = (session: string, direction: "left" | "right" | "up" | "down") =>
      focusNeighbor(tree, leafOf(tree, session), direction)?.sessionId;
    expect(at("a", "right")).toBe("b");
    expect(at("a", "down")).toBe("c");
    expect(at("b", "left")).toBe("a");
    expect(at("b", "down")).toBe("d");
    expect(at("d", "up")).toBe("b");
    expect(at("c", "right")).toBe("d");
    expect(at("a", "left")).toBeUndefined();
    expect(at("a", "up")).toBeUndefined();
  });

  it("prefers the pane sharing the longest edge", () => {
    // a | (b / c): left of c is a, right of a is b (the upper, first match).
    let tree: PaneNode = paneLeaf("a");
    tree = splitPane(tree, leafOf(tree, "a"), "horizontal", "b");
    tree = splitPane(tree, leafOf(tree, "b"), "vertical", "c");
    expect(focusNeighbor(tree, leafOf(tree, "c"), "left")?.sessionId).toBe("a");
    expect(focusNeighbor(tree, leafOf(tree, "a"), "right")?.sessionId).toBe("b");
    expect(focusNeighbor(tree, leafOf(tree, "b"), "down")?.sessionId).toBe("c");
  });

  it("shows a session in its existing pane or in the focused pane", () => {
    let tree: PaneNode = paneLeaf("a");
    tree = splitPane(tree, leafOf(tree, "a"), "horizontal", "b");
    const existing = showSession(tree, leafOf(tree, "a"), "b");
    expect(existing.tree).toBe(tree);
    expect(existing.leafId).toBe(leafOf(tree, "b"));
    const replaced = showSession(tree, leafOf(tree, "b"), "c");
    expect(sessions(replaced.tree)).toEqual(["a", "c"]);
    expect(replaced.leafId).toBe(leafOf(tree, "b"));
    const noFocus = showSession(tree, "gone", "c");
    expect(sessions(noFocus.tree)).toEqual(["c", "b"]);
  });

  it("renames a draft's pane and prunes panes of removed sessions", () => {
    const tree = grid();
    expect(sessions(replacePaneSession(tree, "b", "saved-b"))).toEqual([
      "a",
      "saved-b",
      "c",
      "d",
    ]);
    const pruned = prunePanes(tree, new Set(["a", "d"]));
    expect(pruned && sessions(pruned)).toEqual(["a", "d"]);
    expect(prunePanes(tree, new Set(["a", "b", "c", "d"]))).toBe(tree);
    expect(prunePanes(tree, new Set())).toBeNull();
  });

  it("round-trips a layout through storage", () => {
    const tree = grid();
    const focusedLeafId = leafOf(tree, "c");
    savePaneLayout("C:/Project", { tree, focusedLeafId });
    expect(localStorage.getItem(paneStorageKey("C:/Project"))).not.toBeNull();
    expect(loadPaneLayout("C:/Project")).toEqual({ tree, focusedLeafId });
    expect(loadPaneLayout("C:/Other")).toBeNull();
  });

  it("rejects corrupt stored layouts", () => {
    const leaf = paneLeaf("a");
    const valid = serializePaneLayout({ tree: leaf, focusedLeafId: leaf.id });
    expect(parsePaneLayout(valid)?.tree).toEqual(leaf);
    expect(parsePaneLayout(null)).toBeNull();
    expect(parsePaneLayout("{not json")).toBeNull();
    expect(parsePaneLayout(JSON.stringify({ version: 2, tree: leaf }))).toBeNull();
    expect(
      parsePaneLayout(JSON.stringify({ version: 1, tree: { kind: "leaf", id: "x" } })),
    ).toBeNull();
    expect(
      parsePaneLayout(
        JSON.stringify({
          version: 1,
          tree: { kind: "split", id: "s", direction: "diagonal", a: leaf, b: paneLeaf("b") },
        }),
      ),
    ).toBeNull();
    // One session in two panes, and duplicate node ids.
    expect(
      parsePaneLayout(
        JSON.stringify({
          version: 1,
          tree: { kind: "split", id: "s", direction: "vertical", a: leaf, b: paneLeaf("a") },
        }),
      ),
    ).toBeNull();
    expect(
      parsePaneLayout(
        JSON.stringify({
          version: 1,
          tree: { kind: "split", id: "s", direction: "vertical", a: leaf, b: leaf },
        }),
      ),
    ).toBeNull();
    // More than four panes.
    let five: PaneNode = grid();
    const extra = paneLeaf("e");
    five = { kind: "split", id: "five", direction: "horizontal", a: five, b: extra };
    expect(parsePaneLayout(serializePaneLayout({ tree: five, focusedLeafId: null }))).toBeNull();
    // A focus that names no pane is dropped, the layout kept.
    expect(
      parsePaneLayout(serializePaneLayout({ tree: leaf, focusedLeafId: "gone" }))
        ?.focusedLeafId,
    ).toBeNull();
    localStorage.setItem(paneStorageKey("C:/Broken"), "][");
    expect(loadPaneLayout("C:/Broken")).toBeNull();
  });
});
