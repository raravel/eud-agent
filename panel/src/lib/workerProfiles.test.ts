import { describe, expect, it } from "vitest";
import { isWorkerMessage } from "./protocol";

function event(profile: unknown) {
  return { type: "worker", sessionId: "parent", worker: {
    id: "worker", childSessionId: "child", parentRequestId: "request",
    title: "조사", goal: "근거 확인", status: { kind: "queued" },
    commits: [], changedFiles: [], reported: false, createdAt: 1, updatedAt: 2,
    ...(profile === undefined ? {} : { profile }),
  } };
}

describe("worker role event boundary", () => {
  it.each([undefined, "research", "map_research", "implementation"])(
    "accepts the %s role and legacy creation events", (profile) => {
      expect(isWorkerMessage(event(profile))).toBe(true);
    },
  );
  it.each([null, {}, "planner", 1])("refuses an invalid role %s", (profile) => {
    expect(isWorkerMessage(event(profile))).toBe(false);
  });
});
