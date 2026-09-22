/**
 * `delegation` inbound guard: a session-scoped `delegate_read` child lifecycle
 * passes the dispatch gate with optional error/usage, and a malformed status or
 * missing identity fails it.
 */
import { describe, expect, it } from "vitest";
import {
  SERVER_MESSAGE_TYPES,
  isAgentEventMessage,
  isDelegationMessage,
  isServerMessage,
  isTeamTaskMessage,
} from "./protocol";

const base = {
  type: "delegation",
  sessionId: "session-a",
  requestId: "req-1",
  parentRunId: 7,
  childRunId: 8,
  goal: "P1 체력 저장 위치 찾기",
  status: "running",
  toolCalls: 0,
  elapsedMs: 0,
};

describe("delegation message guard", () => {
  it("accepts every lifecycle status with optional error and usage", () => {
    expect(isDelegationMessage(base)).toBe(true);
    expect(isServerMessage(base)).toBe(true);
    expect(SERVER_MESSAGE_TYPES).toContain("delegation");
    expect(
      isDelegationMessage({
        ...base,
        status: "failed",
        toolCalls: 3,
        elapsedMs: 240_000,
        error: "위임된 읽기 작업을 완료하지 못했습니다: 시간 초과",
      }),
    ).toBe(true);
    expect(
      isDelegationMessage({
        ...base,
        status: "completed",
        usage: {
          last: {
            inputTokens: 1,
            cachedInputTokens: 0,
            cacheWriteInputTokens: 0,
            outputTokens: 1,
            reasoningOutputTokens: 0,
            totalTokens: 2,
          },
          total: {
            inputTokens: 1,
            cachedInputTokens: 0,
            cacheWriteInputTokens: 0,
            outputTokens: 1,
            reasoningOutputTokens: 0,
            totalTokens: 2,
          },
          modelContextWindow: null,
        },
      }),
    ).toBe(true);
  });

  it("rejects an unknown status, a missing run id, or a malformed usage", () => {
    expect(isDelegationMessage({ ...base, status: "done" })).toBe(false);
    expect(isDelegationMessage({ ...base, childRunId: "8" })).toBe(false);
    expect(isDelegationMessage({ ...base, sessionId: "" })).toBe(false);
    expect(isDelegationMessage({ ...base, usage: { last: {} } })).toBe(false);
    expect(isDelegationMessage({ ...base, error: 1 })).toBe(false);
  });

  it("lets a child's tool event carry its delegation run id", () => {
    expect(
      isAgentEventMessage({
        type: "agent_event",
        sessionId: "session-a",
        kind: "tool_call",
        detail: "read_file",
        data: { callId: "c1", args: "{}", delegationRunId: 8 },
      }),
    ).toBe(true);
  });
});

const teamTask = {
  id: "task-1",
  parentRequestId: "req-1",
  mapSessionId: "map-1",
  mapRequestId: "map-abc",
  goal: "공허 지형",
  layers: ["terrain"],
  selectionIds: ["sel-1"],
  locationIds: [3],
  sourceMapSha256AtCreate: "a".repeat(64),
  status: { kind: "candidate_ready" },
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
  createdAt: 1,
  updatedAt: 2,
};

describe("team task message guard", () => {
  it("accepts a persisted task with every status shape", () => {
    const message = { type: "team_task", sessionId: "session-a", task: teamTask };
    expect(isTeamTaskMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
    expect(SERVER_MESSAGE_TYPES).toContain("team_task");
    expect(
      isTeamTaskMessage({
        ...message,
        task: { ...teamTask, candidate: undefined, status: { kind: "failed", reason: "stale" } },
      }),
    ).toBe(true);
    expect(
      isTeamTaskMessage({
        ...message,
        task: { ...teamTask, status: { kind: "applied" }, appliedSourceSha256: "c".repeat(64) },
      }),
    ).toBe(true);
  });

  it("accepts an engine-started continuation only with its turn id and text", () => {
    const message = { type: "team_task", sessionId: "session-a", task: teamTask };
    expect(
      isTeamTaskMessage({
        ...message,
        continuation: { clientTurnId: "turn-1", text: "이어서 진행해 주세요." },
      }),
    ).toBe(true);
    expect(isTeamTaskMessage({ ...message, continuation: { text: "x" } })).toBe(false);
    expect(isTeamTaskMessage({ ...message, continuation: "turn-1" })).toBe(false);
  });

  it("rejects a failed status without a reason, an unknown layer, or a missing id", () => {
    const message = { type: "team_task", sessionId: "session-a", task: teamTask };
    expect(
      isTeamTaskMessage({ ...message, task: { ...teamTask, status: { kind: "failed" } } }),
    ).toBe(false);
    expect(
      isTeamTaskMessage({ ...message, task: { ...teamTask, status: { kind: "done" } } }),
    ).toBe(false);
    expect(isTeamTaskMessage({ ...message, task: { ...teamTask, layers: ["fog"] } })).toBe(false);
    expect(isTeamTaskMessage({ ...message, task: { ...teamTask, id: "" } })).toBe(false);
    expect(isTeamTaskMessage({ ...message, task: { ...teamTask, candidate: { revision: 1 } } })).toBe(false);
  });
});
