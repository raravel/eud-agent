/**
 * Split-pane layout of the chat tab: a binary tree whose leaves each show one
 * session. Pure data operations; App owns the state and persists one layout
 * per root session (a session and the workers it started) in localStorage.
 */

export type PaneDirection = "horizontal" | "vertical";
export type PaneFocusDirection = "left" | "right" | "up" | "down";

export interface PaneLeaf {
  kind: "leaf";
  id: string;
  sessionId: string;
}

export interface PaneSplit {
  kind: "split";
  id: string;
  /** `horizontal` puts `a` left of `b`; `vertical` puts `a` above `b`. */
  direction: PaneDirection;
  a: PaneNode;
  b: PaneNode;
}

export type PaneNode = PaneLeaf | PaneSplit;

/** At most this many panes are shown at once. */
export const MAX_PANES = 4;

/** Before layouts were per root session: one layout per project. */
const LEGACY_STORAGE_PREFIX = "eud-agent.panes/1:";
const STORAGE_PREFIX = "eud-agent.panes/2:";
const OWNER_STORAGE_PREFIX = "eud-agent.pane-owner/1:";

let paneSequence = 0;

function newPaneId(prefix: string): string {
  paneSequence += 1;
  const random =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID().slice(0, 8)
      : Math.random().toString(36).slice(2, 10);
  return `${prefix}-${random}-${paneSequence}`;
}

export function paneLeaf(sessionId: string): PaneLeaf {
  return { kind: "leaf", id: newPaneId("pane"), sessionId };
}

/** Leaves in reading order (a before b). */
export function paneLeaves(node: PaneNode): PaneLeaf[] {
  return node.kind === "leaf"
    ? [node]
    : [...paneLeaves(node.a), ...paneLeaves(node.b)];
}

export function findPaneBySession(
  node: PaneNode,
  sessionId: string,
): PaneLeaf | undefined {
  return paneLeaves(node).find((leaf) => leaf.sessionId === sessionId);
}

function mapLeaves(node: PaneNode, apply: (leaf: PaneLeaf) => PaneNode): PaneNode {
  if (node.kind === "leaf") return apply(node);
  const a = mapLeaves(node.a, apply);
  const b = mapLeaves(node.b, apply);
  return a === node.a && b === node.b ? node : { ...node, a, b };
}

/**
 * Split `leafId` in two: the existing pane keeps its place (left or top) and
 * `sessionId` opens beside it. Unchanged at {@link MAX_PANES} panes or when
 * the session is already shown.
 */
export function splitPane(
  tree: PaneNode,
  leafId: string,
  direction: PaneDirection,
  sessionId: string,
): PaneNode {
  if (paneLeaves(tree).length >= MAX_PANES) return tree;
  if (findPaneBySession(tree, sessionId)) return tree;
  return mapLeaves(tree, (leaf) =>
    leaf.id === leafId
      ? {
          kind: "split",
          id: newPaneId("split"),
          direction,
          a: leaf,
          b: paneLeaf(sessionId),
        }
      : leaf,
  );
}

/** Remove `leafId`; its sibling takes the split's place. The last pane stays. */
export function closePane(tree: PaneNode, leafId: string): PaneNode {
  if (tree.kind === "leaf") return tree;
  const remove = (node: PaneNode): PaneNode | null => {
    if (node.kind === "leaf") return node.id === leafId ? null : node;
    const a = remove(node.a);
    const b = remove(node.b);
    if (a === null) return b;
    if (b === null) return a;
    return a === node.a && b === node.b ? node : { ...node, a, b };
  };
  return remove(tree) ?? tree;
}

/** The first pane of the subtree that takes `leafId`'s place when it closes. */
export function paneSibling(tree: PaneNode, leafId: string): PaneLeaf | undefined {
  if (tree.kind === "leaf") return undefined;
  if (tree.a.kind === "leaf" && tree.a.id === leafId) return paneLeaves(tree.b)[0];
  if (tree.b.kind === "leaf" && tree.b.id === leafId) return paneLeaves(tree.a)[0];
  return paneSibling(tree.a, leafId) ?? paneSibling(tree.b, leafId);
}

/** Replace the session a pane shows (a draft's id becomes its saved id). */
export function replacePaneSession(
  tree: PaneNode,
  fromSessionId: string,
  toSessionId: string,
): PaneNode {
  return mapLeaves(tree, (leaf) =>
    leaf.sessionId === fromSessionId ? { ...leaf, sessionId: toSessionId } : leaf,
  );
}

/**
 * Show `sessionId`: the pane already showing it, else the focused pane (or
 * the first pane when `focusedLeafId` is gone) switches to it. Returns the
 * tree and the pane that now shows the session.
 */
export function showSession(
  tree: PaneNode,
  focusedLeafId: string | null,
  sessionId: string,
): { tree: PaneNode; leafId: string } {
  const existing = findPaneBySession(tree, sessionId);
  if (existing) return { tree, leafId: existing.id };
  const leaves = paneLeaves(tree);
  const target = leaves.find((leaf) => leaf.id === focusedLeafId) ?? leaves[0];
  return {
    tree: mapLeaves(tree, (leaf) =>
      leaf.id === target.id ? { ...leaf, sessionId } : leaf,
    ),
    leafId: target.id,
  };
}

/** Close every pane whose session no longer exists; `null` when none remain. */
export function prunePanes(
  tree: PaneNode,
  existingSessionIds: ReadonlySet<string>,
): PaneNode | null {
  if (tree.kind === "leaf") {
    return existingSessionIds.has(tree.sessionId) ? tree : null;
  }
  const a = prunePanes(tree.a, existingSessionIds);
  const b = prunePanes(tree.b, existingSessionIds);
  if (a === null) return b;
  if (b === null) return a;
  return a === tree.a && b === tree.b ? tree : { ...tree, a, b };
}

interface PaneRect {
  leaf: PaneLeaf;
  left: number;
  top: number;
  right: number;
  bottom: number;
}

function paneRects(
  node: PaneNode,
  left = 0,
  top = 0,
  right = 1,
  bottom = 1,
): PaneRect[] {
  if (node.kind === "leaf") return [{ leaf: node, left, top, right, bottom }];
  if (node.direction === "horizontal") {
    const middle = (left + right) / 2;
    return [
      ...paneRects(node.a, left, top, middle, bottom),
      ...paneRects(node.b, middle, top, right, bottom),
    ];
  }
  const middle = (top + bottom) / 2;
  return [
    ...paneRects(node.a, left, top, right, middle),
    ...paneRects(node.b, left, middle, right, bottom),
  ];
}

const EPSILON = 1e-9;

function overlap(fromA: number, toA: number, fromB: number, toB: number): number {
  return Math.min(toA, toB) - Math.max(fromA, fromB);
}

/**
 * The pane next to `leafId` in `direction`, laid out as equal splits: the
 * nearest pane on that side that shares an edge span, preferring the largest
 * shared span. `undefined` at the edge.
 */
export function focusNeighbor(
  tree: PaneNode,
  leafId: string,
  direction: PaneFocusDirection,
): PaneLeaf | undefined {
  const rects = paneRects(tree);
  const from = rects.find((rect) => rect.leaf.id === leafId);
  if (!from) return undefined;
  let best: { rect: PaneRect; distance: number; shared: number } | undefined;
  for (const rect of rects) {
    if (rect.leaf.id === leafId) continue;
    let distance: number;
    let shared: number;
    switch (direction) {
      case "left":
        distance = from.left - rect.right;
        shared = overlap(from.top, from.bottom, rect.top, rect.bottom);
        break;
      case "right":
        distance = rect.left - from.right;
        shared = overlap(from.top, from.bottom, rect.top, rect.bottom);
        break;
      case "up":
        distance = from.top - rect.bottom;
        shared = overlap(from.left, from.right, rect.left, rect.right);
        break;
      case "down":
        distance = rect.top - from.bottom;
        shared = overlap(from.left, from.right, rect.left, rect.right);
        break;
    }
    if (distance < -EPSILON || shared <= EPSILON) continue;
    if (
      !best ||
      distance < best.distance - EPSILON ||
      (Math.abs(distance - best.distance) <= EPSILON && shared > best.shared + EPSILON)
    ) {
      best = { rect, distance, shared };
    }
  }
  return best?.rect.leaf;
}

/** The localStorage key of the layout owned by one root session. */
export function paneStorageKey(projectPath: string, ownerSessionId: string): string {
  return `${STORAGE_PREFIX}${JSON.stringify([projectPath, ownerSessionId])}`;
}

export interface StoredPaneLayout {
  tree: PaneNode;
  /** The pane that had focus when the layout was saved. */
  focusedLeafId: string | null;
}

export function serializePaneLayout(layout: StoredPaneLayout): string {
  return JSON.stringify({ version: 1, ...layout });
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseNode(value: unknown, ids: Set<string>, depth: number): PaneNode | null {
  if (!isObject(value) || depth > MAX_PANES || typeof value.id !== "string") return null;
  if (!value.id || ids.has(value.id)) return null;
  ids.add(value.id);
  if (value.kind === "leaf") {
    return typeof value.sessionId === "string" && value.sessionId
      ? { kind: "leaf", id: value.id, sessionId: value.sessionId }
      : null;
  }
  if (
    value.kind !== "split" ||
    (value.direction !== "horizontal" && value.direction !== "vertical")
  ) {
    return null;
  }
  const a = parseNode(value.a, ids, depth + 1);
  const b = parseNode(value.b, ids, depth + 1);
  if (!a || !b) return null;
  return { kind: "split", id: value.id, direction: value.direction, a, b };
}

/**
 * Parse a stored layout. Anything malformed (bad JSON, unknown shape,
 * duplicate ids, more than {@link MAX_PANES} panes, one session in two panes)
 * yields `null`, and the caller starts from a single pane.
 */
export function parsePaneLayout(raw: string | null): StoredPaneLayout | null {
  if (!raw) return null;
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!isObject(value) || value.version !== 1) return null;
  const tree = parseNode(value.tree, new Set(), 0);
  if (!tree) return null;
  const leaves = paneLeaves(tree);
  if (leaves.length > MAX_PANES) return null;
  if (new Set(leaves.map((leaf) => leaf.sessionId)).size !== leaves.length) return null;
  const focusedLeafId =
    typeof value.focusedLeafId === "string" &&
    leaves.some((leaf) => leaf.id === value.focusedLeafId)
      ? value.focusedLeafId
      : null;
  return { tree, focusedLeafId };
}

export function loadPaneLayout(
  projectPath: string,
  ownerSessionId: string,
): StoredPaneLayout | null {
  try {
    return parsePaneLayout(localStorage.getItem(paneStorageKey(projectPath, ownerSessionId)));
  } catch {
    return null;
  }
}

export function savePaneLayout(
  projectPath: string,
  ownerSessionId: string,
  layout: StoredPaneLayout,
): void {
  try {
    localStorage.setItem(
      paneStorageKey(projectPath, ownerSessionId),
      serializePaneLayout(layout),
    );
  } catch {
    // Layout persistence is a convenience; the current window keeps its panes.
  }
}

export function removePaneLayout(projectPath: string, ownerSessionId: string): void {
  try {
    localStorage.removeItem(paneStorageKey(projectPath, ownerSessionId));
  } catch {
    // A leftover layout is pruned or ignored when it is next loaded.
  }
}

/** The project's single layout from before layouts were per root session. */
export function loadLegacyPaneLayout(projectPath: string): StoredPaneLayout | null {
  try {
    return parsePaneLayout(localStorage.getItem(`${LEGACY_STORAGE_PREFIX}${projectPath}`));
  } catch {
    return null;
  }
}

/** The root session whose layout was last on screen in the project. */
export function loadPaneOwner(projectPath: string): string | null {
  try {
    return localStorage.getItem(`${OWNER_STORAGE_PREFIX}${projectPath}`) || null;
  } catch {
    return null;
  }
}

export function savePaneOwner(projectPath: string, ownerSessionId: string): void {
  try {
    localStorage.setItem(`${OWNER_STORAGE_PREFIX}${projectPath}`, ownerSessionId);
  } catch {
    // The next start falls back to the first session's layout.
  }
}

/** localStorage for split sizes, tolerant of blocked storage. */
export const paneSizeStorage = {
  getItem(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  setItem(key: string, value: string): void {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Split sizes fall back to even halves next time.
    }
  },
};
