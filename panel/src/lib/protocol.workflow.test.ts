/**
 * `workflow` inbound guard: the session-scoped stage snapshot is accepted with
 * optional artifacts, and malformed stages/artifacts fail the dispatch gate.
 */
import { describe, expect, it } from "vitest";
import {
  CLIENT_MESSAGE_TYPES,
  SERVER_MESSAGE_TYPES,
  isServerMessage,
  isWorkflowMessage,
} from "./protocol";

const base = {
  type: "workflow",
  sessionId: "session-a",
  requestId: "req-1",
  stage: "plan_review",
  route: "pipeline",
  goal: "미네랄 지급 트리거 추가",
  acceptanceCriteria: ["게임 시작 시 미네랄 1000 지급"],
  research: {
    path: ".eud-agent/workspace/research/req-1.md",
    sha256: "a".repeat(64),
    summary: "요약",
  },
  plan: {
    path: ".eud-agent/workspace/plans/req-1.md",
    revision: 1,
    sha256: "b".repeat(64),
    title: "계획",
    acceptanceCriteria: ["게임 시작 시 미네랄 1000 지급"],
    criticVerdict: "approve",
    criticSummary: "문제 없음",
    deep: false,
    iterations: 1,
  },
  critiqueRounds: 1,
  verifyAttempts: 0,
  deepPlanning: false,
};

describe("workflow message guard", () => {
  it("registers the workflow event and the resume/restart commands", () => {
    expect(SERVER_MESSAGE_TYPES).toContain("workflow");
    expect(CLIENT_MESSAGE_TYPES).toEqual(
      expect.arrayContaining(["workflow_resume", "workflow_restart"]),
    );
  });

  it("accepts a complete snapshot and routes it through isServerMessage", () => {
    expect(isWorkflowMessage(base)).toBe(true);
    expect(isServerMessage(base)).toBe(true);
  });

  it("accepts a minimal snapshot without optional artifacts", () => {
    expect(
      isWorkflowMessage({
        type: "workflow",
        sessionId: "session-a",
        requestId: "req-1",
        stage: "triage",
        acceptanceCriteria: [],
        critiqueRounds: 0,
        verifyAttempts: 0,
        deepPlanning: true,
      }),
    ).toBe(true);
  });

  it("accepts an interrupted snapshot with a verdict", () => {
    expect(
      isWorkflowMessage({
        ...base,
        stage: "interrupted",
        interruptedStage: "verifying",
        verifyAttempts: 1,
        verdict: {
          verdict: "fail",
          summary: "미충족",
          path: ".eud-agent/workspace/verify/req-1.md",
          sha256: "c".repeat(64),
          unmet: ["빌드 통과"],
        },
      }),
    ).toBe(true);
  });

  it("accepts the cancelled terminal stage", () => {
    expect(isWorkflowMessage({ ...base, stage: "cancelled" })).toBe(true);
  });

  it("rejects unknown stages, routes, and malformed artifacts", () => {
    expect(isWorkflowMessage({ ...base, stage: "reviewing" })).toBe(false);
    expect(isWorkflowMessage({ ...base, route: "auto" })).toBe(false);
    expect(isWorkflowMessage({ ...base, sessionId: "" })).toBe(false);
    expect(isWorkflowMessage({ ...base, acceptanceCriteria: "x" })).toBe(false);
    expect(
      isWorkflowMessage({ ...base, plan: { ...base.plan, criticVerdict: "maybe" } }),
    ).toBe(false);
    expect(
      isWorkflowMessage({ ...base, research: { path: "p", sha256: "s" } }),
    ).toBe(false);
    expect(isWorkflowMessage({ ...base, deepPlanning: "yes" })).toBe(false);
  });
});
