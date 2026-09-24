/**
 * EPS → Map team tasks in the store: `team_task` events replace by id and
 * hydration replaces the whole list. There is no card; the conversation log
 * and the Map window carry the task's state.
 */
import { describe, expect, it } from "vitest";
import { createPanelStore } from "./store";
import type { TeamTask } from "@/lib/ipc";

function readyWithProject() {
  const store = createPanelStore();
  store.wsOpen();
  store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
  return store;
}

function task(id: string, status: TeamTask["status"], updatedAt: number): TeamTask {
  return {
    id,
    parentRequestId: "req-1",
    mapSessionId: "map-1",
    goal: `goal ${id}`,
    layers: ["terrain"],
    sourceMapSha256AtCreate: "a".repeat(64),
    status,
    createdAt: updatedAt,
    updatedAt,
  };
}

describe("team tasks", () => {
  it("replaces a task by id and keeps the rest", () => {
    const store = readyWithProject();
    expect(store.getState().teamTasks).toEqual([]);

    store.teamTaskReceived(task("t1", { kind: "running" }, 1));
    store.teamTaskReceived(task("t2", { kind: "queued" }, 2));
    store.teamTaskReceived({
      ...task("t1", { kind: "candidate_ready" }, 3),
      candidate: {
        revision: 1,
        revisionKey: "r1:abc",
        mapSha256: "b".repeat(64),
        summary: "지형 40칸",
        terrainCells: 40,
        units: 0,
        buildings: 0,
        doodads: 0,
        sprites: 0,
        locations: 0,
      },
    });
    const tasks = store.getState().teamTasks;
    expect(tasks.map((entry) => entry.id)).toEqual(["t1", "t2"]);
    expect(tasks[0].candidate?.revision).toBe(1);

    // Sending the next message keeps the tasks for the `[map tasks]` note.
    store.chatSent();
    expect(store.getState().teamTasks).toHaveLength(2);
  });

  it("hydrates the whole list from the session record", () => {
    const store = readyWithProject();
    store.teamTaskReceived(task("stale", { kind: "running" }, 1));
    store.setTeamTasks([task("done", { kind: "applied" }, 1)]);
    expect(store.getState().teamTasks.map((entry) => entry.id)).toEqual(["done"]);
  });
});
