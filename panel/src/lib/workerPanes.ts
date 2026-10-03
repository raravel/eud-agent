import {
  closePane, findPaneBySession, MAX_PANES, paneLeaves, replacePaneSession, splitPane,
  type PaneNode,
} from "./paneLayout";
import type { WorkerRecord } from "./protocol";

export interface WorkerPaneRequest {
  readonly parentSessionId: string;
  readonly childSessionId: string;
  readonly focusedLeafId: string | null;
  readonly workers: readonly WorkerRecord[];
}

export function closeWorkerPane(tree: PaneNode, worker: Pick<WorkerPaneRequest,
  "parentSessionId" | "childSessionId">): PaneNode {
  const child = findPaneBySession(tree, worker.childSessionId);
  if (!child) return tree;
  return tree.kind === "leaf"
    ? replacePaneSession(tree, worker.childSessionId, worker.parentSessionId)
    : closePane(tree, child.id);
}

export function showWorkerPane(tree: PaneNode, request: WorkerPaneRequest): PaneNode {
  if (findPaneBySession(tree, request.childSessionId)) return tree;
  const leaves = paneLeaves(tree);
  if (leaves.length >= MAX_PANES) {
    const reusable = leaves.find((leaf) =>
      leaf.id !== request.focusedLeafId && request.workers.some((worker) =>
        worker.childSessionId === leaf.sessionId &&
        worker.status.kind !== "queued" && worker.status.kind !== "running"),
    );
    return reusable
      ? replacePaneSession(tree, reusable.sessionId, request.childSessionId)
      : tree;
  }
  let host = findPaneBySession(tree, request.parentSessionId)
    ?? leaves.find((leaf) => leaf.id === request.focusedLeafId)
    ?? leaves[0];
  if (leaves.length >= 3) {
    for (const leaf of leaves) {
      if (request.workers.some((worker) => worker.childSessionId === leaf.sessionId)) host = leaf;
    }
  }
  return host ? splitPane(tree, host.id,
    leaves.length === 1 ? "horizontal" : "vertical", request.childSessionId) : tree;
}
