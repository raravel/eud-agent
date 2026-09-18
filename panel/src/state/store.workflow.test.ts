/**
 * Staged-workflow store contracts (features/staged-workflow-plan.md ## Phase 2
 * Panel): every `workflow` stage projects onto a phase, busy stages gate
 * sending like `thinking`, plan review survives a transport re-open, and the
 * interrupted controls transition through resume/restart.
 */
import { describe, it, expect } from "vitest";
import {
  createPanelStore,
  isBusyPhase,
  phaseForWorkflowStage,
  type Phase,
} from "@/state/store";
import type { WorkflowEvent, WorkflowStage } from "@/lib/ipc";

function readyWithProject() {
  const store = createPanelStore();
  store.wsOpen();
  store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
  return store;
}

function workflow(overrides: Partial<WorkflowEvent> = {}): WorkflowEvent {
  return {
    requestId: "req-1",
    stage: "triage",
    acceptanceCriteria: [],
    critiqueRounds: 0,
    verifyAttempts: 0,
    deepPlanning: false,
    ...overrides,
  };
}

const research = {
  path: ".eud-agent/workspace/research/req-1.md",
  sha256: "a".repeat(64),
  summary: "트리거 3개와 MainFile 구성을 확인했습니다.",
};

const verdictFail = {
  verdict: "fail" as const,
  summary: "빌드는 통과했지만 수용 기준 하나가 미충족입니다.",
  path: ".eud-agent/workspace/verify/req-1.md",
  sha256: "b".repeat(64),
  unmet: ["미네랄 지급 트리거가 모든 플레이어에게 적용되어야 함"],
};

describe("stage → phase projection", () => {
  const table: ReadonlyArray<[WorkflowStage, Phase, boolean]> = [
    ["triage", "thinking", true],
    ["clarify", "thinking", true],
    ["research", "research", true],
    ["planning", "planning", true],
    ["critique", "planning", true],
    ["plan_review", "plan_review", false],
    ["executing", "thinking", true],
    ["verifying", "verifying", true],
    ["changeset_review", "changeset_review", false],
    ["interrupted", "interrupted", false],
    ["cancelled", "ready", false],
    ["done", "ready", false],
    ["failed", "ready", false],
  ];

  for (const [stage, phase, busy] of table) {
    it(`${stage} → ${phase}${busy ? " (busy)" : ""}`, () => {
      expect(phaseForWorkflowStage(stage)).toBe(phase);
      expect(isBusyPhase(phase)).toBe(busy);
      const store = readyWithProject();
      store.chatSent();
      store.workflowReceived(workflow({ stage }));
      const s = store.getState();
      expect(s.phase).toBe(phase);
      expect(s.workflow?.stage).toBe(stage);
      // Busy stages gate sending exactly like thinking; review/interrupted/
      // terminal stages leave the prompt usable.
      expect(s.canSend).toBe(!busy);
    });
  }

  it("keeps an undecided changeset reviewable when done arrives after changeset", () => {
    const store = readyWithProject();
    store.chatSent();
    store.workflowReceived(workflow({ stage: "executing", route: "pipeline" }));
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    store.workflowReceived(
      workflow({ stage: "changeset_review", route: "pipeline" }),
    );
    expect(store.getState().phase).toBe("changeset_review");
    store.workflowReceived(workflow({ stage: "done", route: "pipeline" }));
    expect(store.getState().phase).toBe("changeset_review");
    expect(store.getState().changeset?.request_id).toBe("req-1");
  });

  it("a busy stage accepts streamed agent events after a restore without chatSent", () => {
    const store = readyWithProject();
    store.workflowReceived(workflow({ stage: "executing", route: "pipeline" }));
    store.agentEvent("delta", "진행 중");
    expect(store.getState().turn.answer).toBe("진행 중");
  });

  it("failed clears the plan and returns to ready", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.workflowReceived(
      workflow({ stage: "failed", route: "pipeline", error: "critic timeout" }),
    );
    const s = store.getState();
    expect(s.phase).toBe("ready");
    expect(s.plan).toBeNull();
    expect(s.workflow?.error).toBe("critic timeout");
  });
});

describe("workflow snapshot lifecycle", () => {
  it("a new chat clears the previous request's snapshot", () => {
    const store = readyWithProject();
    store.chatSent();
    store.workflowReceived(workflow({ stage: "done", route: "answer" }));
    expect(store.getState().workflow).not.toBeNull();
    store.chatSent();
    expect(store.getState().workflow).toBeNull();
  });

  it("a cancelled snapshot survives cancelSent and restarts cleanly", () => {
    const store = readyWithProject();
    store.chatSent();
    store.workflowReceived(workflow({ stage: "research", route: "pipeline", research }));
    store.workflowReceived(
      workflow({ stage: "cancelled", interruptedStage: "research", route: "pipeline", research }),
    );
    store.cancelSent();
    expect(store.getState().phase).toBe("ready");
    expect(store.getState().workflow?.stage).toBe("cancelled");
    expect(store.getState().workflow?.research).toEqual(research);
    store.workflowRestartSent();
    expect(store.getState().phase).toBe("thinking");
    expect(store.getState().workflow).toBeNull();
  });

  it("cancel drops the snapshot together with the plan", () => {
    const store = readyWithProject();
    store.chatSent();
    store.workflowReceived(workflow({ stage: "research", route: "pipeline" }));
    store.cancelSent();
    const s = store.getState();
    expect(s.phase).toBe("ready");
    expect(s.workflow).toBeNull();
  });

  it("archives research and the verdict into the log once each while a turn is in flight", () => {
    const store = readyWithProject();
    store.chatSent();
    store.workflowReceived(
      workflow({ stage: "research", route: "pipeline", research }),
    );
    store.workflowReceived(
      workflow({ stage: "planning", route: "pipeline", research }),
    );
    const texts = store.getState().log.map((entry) => entry.text);
    expect(texts.filter((text) => text.includes(research.summary))).toHaveLength(1);
    expect(texts.some((text) => text.includes(research.path))).toBe(true);

    store.planReceived("# plan", 1);
    store.planApproveSent();
    store.workflowReceived(
      workflow({
        stage: "executing",
        route: "pipeline",
        research,
        verifyAttempts: 1,
        verdict: verdictFail,
      }),
    );
    const verdictRows = store
      .getState()
      .log.filter((entry) => entry.text.includes(verdictFail.summary));
    expect(verdictRows).toHaveLength(1);
    expect(verdictRows[0].kind).toBe("warn");
    expect(verdictRows[0].text).toContain(verdictFail.unmet[0]);
  });

  it("does not re-log a hydrated research artifact (no turn in flight)", () => {
    const store = readyWithProject();
    store.workflowReceived(
      workflow({ stage: "plan_review", route: "pipeline", research }),
    );
    expect(
      store.getState().log.some((entry) => entry.text.includes(research.summary)),
    ).toBe(false);
    expect(store.getState().workflow?.research).toEqual(research);
  });
});

describe("plan review restore after reconnect", () => {
  it("a plan event received in ready (after wsOpen) enters plan_review", () => {
    const store = readyWithProject();
    expect(store.getState().phase).toBe("ready");
    store.workflowReceived(workflow({ stage: "plan_review", route: "pipeline" }));
    store.planReceived("# 복구된 계획", 2);
    const s = store.getState();
    expect(s.phase).toBe("plan_review");
    expect(s.plan).toEqual({ markdown: "# 복구된 계획", revision: 2 });
  });

  it("a plan event received while connecting is kept through wsOpen", () => {
    const store = createPanelStore();
    store.planReceived("# 복구된 계획", 1);
    store.wsOpen();
    const s = store.getState();
    expect(s.phase).toBe("plan_review");
    expect(s.plan?.revision).toBe(1);
  });

  it("wsOpen with an in-flight turn still resets and lets the re-emitted plan restore review", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.planApproveSent();
    store.wsConnecting();
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
    expect(store.getState().plan).toBeNull();
    store.planReceived("# plan", 1);
    expect(store.getState().phase).toBe("plan_review");
  });

  it("wsOpen restores an interrupted stage", () => {
    const store = readyWithProject();
    store.workflowReceived(
      workflow({ stage: "interrupted", interruptedStage: "research", route: "pipeline" }),
    );
    store.wsConnecting();
    store.wsOpen();
    expect(store.getState().phase).toBe("interrupted");
  });
});

describe("interrupted controls", () => {
  it("resume enters the interrupted stage's busy phase with a fresh turn", () => {
    const store = readyWithProject();
    store.workflowReceived(
      workflow({ stage: "interrupted", interruptedStage: "verifying", route: "pipeline" }),
    );
    expect(store.getState().canSend).toBe(true);
    store.workflowResumeSent();
    const s = store.getState();
    expect(s.phase).toBe("verifying");
    expect(s.canSend).toBe(false);
    expect(s.workflow?.stage).toBe("interrupted");
    store.agentEvent("delta", "재개");
    expect(store.getState().turn.answer).toBe("재개");
  });

  it("resume without a known interrupted stage lands on thinking", () => {
    const store = readyWithProject();
    store.workflowReceived(workflow({ stage: "interrupted", route: "pipeline" }));
    store.workflowResumeSent();
    expect(store.getState().phase).toBe("thinking");
  });

  it("restart drops the snapshot and plan and re-enters thinking", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.workflowReceived(
      workflow({ stage: "interrupted", interruptedStage: "planning", route: "pipeline" }),
    );
    store.workflowRestartSent();
    const s = store.getState();
    expect(s.phase).toBe("thinking");
    expect(s.workflow).toBeNull();
    expect(s.plan).toBeNull();
    store.workflowReceived(workflow({ stage: "triage" }));
    expect(store.getState().phase).toBe("thinking");
  });
});
