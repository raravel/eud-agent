import { describe, it, expect } from "vitest";
import {
  createPanelStore,
  MAX_LOG_ENTRIES,
  type PanelState,
} from "@/state/store";
import { CLIENT_MESSAGE_TYPES, SERVER_MESSAGE_TYPES } from "@/lib/ipc";

function freshStore() {
  return createPanelStore();
}

/** Drive a store to a ready state with an open project + one file. */
function readyWithProject() {
  const store = freshStore();
  store.wsOpen();
  store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
  return store;
}

describe("initial state", () => {
  it("starts in connecting with an empty log, no project, no plan/changeset", () => {
    const s = freshStore().getState();
    expect(s.phase).toBe("connecting");
    expect(s.log).toEqual([]);
    expect(s.hasProject).toBe(false);
    expect(s.files).toEqual([]);
    expect(s.plan).toBeNull();
    expect(s.changeset).toBeNull();
    expect(s.connected).toBe(false);
  });
});

describe("connection lifecycle transitions (features/06 mermaid)", () => {
  it("connecting -> ready on wsOpen", () => {
    const store = freshStore();
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
    expect(store.getState().connected).toBe(true);
  });

  it("connecting -> retry on wsError", () => {
    const store = freshStore();
    store.wsError();
    expect(store.getState().phase).toBe("retry");
    expect(store.getState().connected).toBe(false);
  });

  it("retry -> connecting on wsConnecting", () => {
    const store = freshStore();
    store.wsError();
    store.wsConnecting();
    expect(store.getState().phase).toBe("connecting");
  });
});

describe("turn transitions (ready <-> thinking -> plan_review|changeset_review)", () => {
  it("ready -> thinking on chatSent", () => {
    const store = readyWithProject();
    store.chatSent();
    expect(store.getState().phase).toBe("thinking");
  });

  it("thinking -> ready on answer (no edits)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.answerReceived("here is your answer");
    expect(store.getState().phase).toBe("ready");
  });

  it("thinking -> plan_review on plan", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    const s = store.getState();
    expect(s.phase).toBe("plan_review");
    expect(s.plan).toEqual({ markdown: "# plan", revision: 1 });
  });

  it("plan_review -> thinking on plan feedback", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.planFeedbackSent();
    expect(store.getState().phase).toBe("thinking");
  });

  it("plan_review -> thinking on plan approve", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.planApproveSent();
    expect(store.getState().phase).toBe("thinking");
  });

  it("thinking -> changeset_review on changeset", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    const s = store.getState();
    expect(s.phase).toBe("changeset_review");
    expect(s.changeset?.request_id).toBe("req-1");
    expect(s.changeset?.items.length).toBe(1);
  });

  it("changeset_review -> ready after a full accept decision", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    // Bulk accept: the server echoes an EMPTY ids array (it does not return the
    // accepted ids); the recorded decision resolves it against all undecided.
    store.decisionSent("accept", "all");
    store.rollbackResult([], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("accepted");
    expect(s.phase).toBe("ready");
  });

  it("changeset_review -> thinking on follow-up chat", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    store.chatSent();
    expect(store.getState().phase).toBe("thinking");
  });
});

describe("plan revision replacement (next plan{revision+1} replaces the card)", () => {
  it("replaces the active plan card with the new revision", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan v1", 1);
    store.planFeedbackSent();
    store.planReceived("# plan v2", 2);
    const s = store.getState();
    expect(s.phase).toBe("plan_review");
    expect(s.plan).toEqual({ markdown: "# plan v2", revision: 2 });
  });
});

describe("reconnect during thinking resets to ready WITH a notice", () => {
  it("a wsOpen mid-thinking lands on ready and logs a notice", () => {
    const store = readyWithProject();
    store.chatSent();
    expect(store.getState().phase).toBe("thinking");
    store.wsConnecting();
    store.wsOpen();
    const s = store.getState();
    expect(s.phase).toBe("ready");
    // The server cancels the turn on reconnect; the panel surfaces a notice.
    const last = s.log[s.log.length - 1];
    expect(last.kind).toBe("warn");
    expect(last.text.length).toBeGreaterThan(0);
  });

  it("a wsOpen mid-plan_review also resets to ready with a notice", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.wsConnecting();
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
  });
});

describe("changeset stays reviewable across reconnect (server-persisted journal)", () => {
  it("keeps the changeset and stays in changeset_review after a reconnect", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    store.wsConnecting();
    store.wsOpen();
    const s = store.getState();
    expect(s.phase).toBe("changeset_review");
    expect(s.changeset?.request_id).toBe("req-1");
  });
});

describe("rollback_result labels items per the RECORDED decision", () => {
  /** Drive to changeset_review with the given items. */
  function reviewing(items: Array<{ id: string; seq: number }>) {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived(
      "req-1",
      items.map((it) => ({
        category: "file",
        kind: "modified",
        path: `${it.id}.eps`,
        id: it.id,
        seq: it.seq,
      })),
    );
    return store;
  }

  it("per-item ACCEPT → 'accepted' (NOT 'rejected'); fully decided → ready", () => {
    const store = reviewing([{ id: "e1", seq: 0 }]);
    // The server routes accept through rollback_result too (ids echoed, ok:true).
    store.decisionSent("accept", ["e1"]);
    store.rollbackResult(["e1"], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("accepted");
    expect(s.pendingDecision).toBeNull();
    expect(s.phase).toBe("ready");
  });

  it("per-item REJECT (ok=true) → 'rejected'; mixed keeps changeset_review", () => {
    const store = reviewing([
      { id: "e1", seq: 0 },
      { id: "e2", seq: 1 },
    ]);
    store.decisionSent("reject", ["e1"]);
    store.rollbackResult(["e1"], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("rejected");
    expect(s.changeset?.decisions["e2"]).toBeUndefined(); // still undecided
    expect(s.phase).toBe("changeset_review");
  });

  it("per-item REJECT (ok=false) → 'failed'; failure keeps the panel open", () => {
    const store = reviewing([{ id: "e1", seq: 0 }]);
    store.decisionSent("reject", ["e1"]);
    store.rollbackResult(["e1"], false);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("failed");
    // A failure does NOT advance to ready even when all items are "decided".
    expect(s.phase).toBe("changeset_review");
  });

  it("BULK accept (server ids:[]) → ALL undecided become 'accepted'; ready", () => {
    const store = reviewing([
      { id: "e1", seq: 0 },
      { id: "e2", seq: 1 },
    ]);
    // Accept-all: the server does NOT echo the accepted ids — it sends ids:[].
    store.decisionSent("accept", "all");
    store.rollbackResult([], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("accepted");
    expect(s.changeset?.decisions["e2"]).toBe("accepted");
    expect(s.phase).toBe("ready");
  });

  it("BULK accept applies only to UNDECIDED items (prior decisions kept)", () => {
    const store = reviewing([
      { id: "e1", seq: 0 },
      { id: "e2", seq: 1 },
    ]);
    // First reject e1...
    store.decisionSent("reject", ["e1"]);
    store.rollbackResult(["e1"], true);
    expect(store.getState().changeset?.decisions["e1"]).toBe("rejected");
    // ...then accept-all the rest (server ids:[]).
    store.decisionSent("accept", "all");
    store.rollbackResult([], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("rejected"); // untouched
    expect(s.changeset?.decisions["e2"]).toBe("accepted");
    expect(s.phase).toBe("ready");
  });

  it("BULK reject (server echoes the real ids) → all 'rejected'; ready", () => {
    const store = reviewing([
      { id: "e1", seq: 0 },
      { id: "e2", seq: 1 },
    ]);
    store.decisionSent("reject", "all");
    // reject DOES return the real journal ids (unlike accept-all).
    store.rollbackResult(["e2", "e1"], true);
    const s = store.getState();
    expect(s.changeset?.decisions["e1"]).toBe("rejected");
    expect(s.changeset?.decisions["e2"]).toBe("rejected");
    expect(s.phase).toBe("ready");
  });

  it("defensive fallback: rollback_result with NO recorded decision", () => {
    const store = reviewing([{ id: "e1", seq: 0 }]);
    // No decisionSent() first (should not happen — the store is the sole sender).
    // The fallback treats it as the legacy reject-shaped reply.
    store.rollbackResult(["e1"], true);
    expect(store.getState().changeset?.decisions["e1"]).toBe("rejected");
    store.rollbackResult(["e1"], false);
    // ok=false fallback → failed.
    const store2 = reviewing([{ id: "z1", seq: 0 }]);
    store2.rollbackResult(["z1"], false);
    expect(store2.getState().changeset?.decisions["z1"]).toBe("failed");
  });
});

describe("dat-group changeset (NO item-level id — completion via property ids)", () => {
  // REPRESENTATIVE: a server dat group carries no item-level id; its ids live on
  // properties[]. The store must derive completion via itemIds (lib/changeset),
  // NOT decisions[it.id] (permanently undefined → never "fully decided").
  function datGroup(objId: number, propIds: string[]) {
    return {
      category: "dat",
      dat: "unit",
      objId,
      properties: propIds.map((id, i) => ({
        property: `p${i}`,
        old: "0",
        new: "1",
        id,
        seq: i,
      })),
    };
  }

  /** Drive to changeset_review with the given (id-less) dat groups. */
  function reviewingDat(groups: Array<ReturnType<typeof datGroup>>) {
    const store = readyWithProject();
    store.chatSent();
    // The ChangesetItem type wants id/seq; production dat groups omit them.
    store.changesetReceived("req-dat", groups as unknown as Parameters<
      typeof store.changesetReceived
    >[1]);
    return store;
  }

  it("per-PROPERTY decisions complete a dat group → ready (was unreachable)", () => {
    const store = reviewingDat([datGroup(76, ["p1", "p2"])]);
    expect(store.getState().phase).toBe("changeset_review");
    // Accept the whole group (ChangesetView dispatches BOTH property ids).
    store.decisionSent("accept", ["p1", "p2"]);
    store.rollbackResult(["p1", "p2"], true);
    const s = store.getState();
    expect(s.changeset?.decisions["p1"]).toBe("accepted");
    expect(s.changeset?.decisions["p2"]).toBe("accepted");
    // Critical: a dat-only changeset now reaches "fully decided".
    expect(s.phase).toBe("ready");
  });

  it("stays in changeset_review until EVERY property of a dat group is decided", () => {
    const store = reviewingDat([datGroup(76, ["p1", "p2"])]);
    store.decisionSent("accept", ["p1"]);
    store.rollbackResult(["p1"], true);
    // p2 still undecided → the group (and changeset) is NOT fully decided.
    expect(store.getState().phase).toBe("changeset_review");
  });

  it("BULK accept (server ids:[]) resolves an id-less dat group → ready", () => {
    const store = reviewingDat([
      datGroup(76, ["p1", "p2"]),
      datGroup(0, ["q1"]),
    ]);
    store.decisionSent("accept", "all");
    // accept-all echoes EMPTY ids; the store resolves against all undecided
    // PROPERTY ids (via itemIds), not it.id.
    store.rollbackResult([], true);
    const s = store.getState();
    expect(s.changeset?.decisions["p1"]).toBe("accepted");
    expect(s.changeset?.decisions["p2"]).toBe("accepted");
    expect(s.changeset?.decisions["q1"]).toBe("accepted");
    expect(s.phase).toBe("ready");
  });

  it("an UNDECIDED dat group stays reviewable across a reconnect", () => {
    const store = reviewingDat([datGroup(76, ["p1", "p2"])]);
    store.wsConnecting(); // drop
    store.wsOpen(); // reconnect: undecided dat group must re-open review
    const s = store.getState();
    expect(s.phase).toBe("changeset_review");
    expect(s.changeset?.request_id).toBe("req-dat");
  });

  it("a FULLY-DECIDED dat group does NOT re-open review on reconnect", () => {
    const store = reviewingDat([datGroup(76, ["p1", "p2"])]);
    store.decisionSent("accept", ["p1", "p2"]);
    store.rollbackResult(["p1", "p2"], true);
    expect(store.getState().phase).toBe("ready");
    store.wsConnecting();
    store.wsOpen();
    // Already fully decided → lands on ready, not changeset_review.
    expect(store.getState().phase).toBe("ready");
  });
});

describe("send gating v2 = connected && hasProject && !busy (no settable target req.)", () => {
  it("allows send when connected with a project, even with zero files", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [] }); // open project, zero files
    const s = store.getState();
    expect(s.hasProject).toBe(true);
    expect(s.canSend).toBe(true); // settable target NOT required (agent picks)
  });

  it("allows send with only non-settable (GUI) files (no settable-target gate)", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "gui.tgui", ftype: "GUI", settable: false }] });
    expect(store.getState().canSend).toBe(true);
  });

  it("blocks send when no project is open", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ error: "no project" });
    const s = store.getState();
    expect(s.hasProject).toBe(false);
    expect(s.canSend).toBe(false);
  });

  it("blocks send while busy (thinking)", () => {
    const store = readyWithProject();
    store.chatSent();
    expect(store.getState().canSend).toBe(false);
  });

  it("allows send during plan_review (the main input IS the feedback channel — EUD-074)", () => {
    // The PlanView feedback textarea is REMOVED (user decision 2026-06-05):
    // typing in the main prompt during plan_review sends plan_feedback{}.
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    expect(store.getState().canSend).toBe(true);
  });

  it("allows send during changeset_review (follow-up chat auto-accepts)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("req-1", [
      { category: "file", kind: "created", path: "x.eps", id: "e1", seq: 0 },
    ]);
    expect(store.getState().canSend).toBe(true);
  });

  it("blocks send when disconnected", () => {
    const store = readyWithProject();
    store.wsError();
    expect(store.getState().canSend).toBe(false);
  });
});

describe("no-project signal via error message (server contract: no list{error})", () => {
  it("error{message:'ERROR: no project'} clears the project + gates send off", () => {
    const store = readyWithProject();
    expect(store.getState().canSend).toBe(true);
    store.errorReceived("ERROR: no project");
    const s = store.getState();
    expect(s.hasProject).toBe(false);
    expect(s.files).toEqual([]);
    expect(s.canSend).toBe(false);
    expect(s.phase).toBe("ready");
  });

  it("an unrelated error does NOT clear an open project", () => {
    const store = readyWithProject();
    store.chatSent();
    store.errorReceived("agent turn failed: boom");
    const s = store.getState();
    expect(s.hasProject).toBe(true);
    expect(s.phase).toBe("ready");
  });
});

describe("status compiling flag (documented status event field)", () => {
  it("stores compiling=true / project from a status event", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: true, project: "MyMap" });
    const s = store.getState();
    expect(s.compiling).toBe(true);
    expect(s.project).toBe("MyMap");
  });

  it("compiling makes canSend false (busy editor)", () => {
    const store = readyWithProject();
    store.applyStatus({ compiling: true, project: "MyMap" });
    expect(store.getState().canSend).toBe(false);
  });
});

describe("event log cap (drop oldest, max 500)", () => {
  it("exposes the cap constant of 500", () => {
    expect(MAX_LOG_ENTRIES).toBe(500);
  });

  it("caps the log at MAX_LOG_ENTRIES, dropping the oldest", () => {
    const store = freshStore();
    for (let i = 0; i < MAX_LOG_ENTRIES + 50; i++) {
      store.log("info", `line ${i}`);
    }
    const log = store.getState().log;
    expect(log.length).toBe(MAX_LOG_ENTRIES);
    expect(log[0].text).toBe("line 50");
  });
});

describe("subscribe / notify", () => {
  it("notifies subscribers on state change and supports unsubscribe", () => {
    const store = freshStore();
    let count = 0;
    const unsub = store.subscribe(() => {
      count += 1;
    });
    store.wsOpen();
    store.log("info", "hi");
    expect(count).toBeGreaterThanOrEqual(2);
    const after = count;
    unsub();
    store.log("info", "bye");
    expect(count).toBe(after);
  });
});

// A static contract guard: the v1 message type literals must be ABSENT from the
// protocol's exported discriminant sets (features/05: instruct/apply/code/applied
// REMOVED entirely; no compat shim).
describe("v1 protocol literals are absent (no compat shim)", () => {
  it("client message types exclude instruct/apply", () => {
    expect(CLIENT_MESSAGE_TYPES).not.toContain("instruct");
    expect(CLIENT_MESSAGE_TYPES).not.toContain("apply");
  });

  it("server message types exclude code/applied", () => {
    expect(SERVER_MESSAGE_TYPES).not.toContain("code");
    expect(SERVER_MESSAGE_TYPES).not.toContain("applied");
  });

  it("includes the v2 client + server message types", () => {
    expect(CLIENT_MESSAGE_TYPES).toEqual(
      expect.arrayContaining([
        "chat",
        "plan_feedback",
        "plan_approve",
        "changeset_decision",
        "cancel",
        "reset",
        "status",
        "list",
      ]),
    );
    expect(SERVER_MESSAGE_TYPES).toEqual(
      expect.arrayContaining([
        "agent_event",
        "answer",
        "plan",
        "changeset",
        "rollback_result",
        "error",
        "status",
        "progress",
        "list",
      ]),
    );
  });
});

// ---- EUD-065: per-turn streaming buffers (reasoning / delta / tools) ----
// The store accumulates the EUD-063 streamed agent_event deltas into a per-turn
// `turn` buffer so the AI-Elements surfaces (Reasoning / Response / Tool) render
// live and reset per turn. Raw kind identifiers MUST NOT leak into the log.
describe("agentEvent streaming buffers (EUD-065 / features/06)", () => {
  it("accumulates reasoning deltas into turn.reasoning", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("reasoning", "먼저 ");
    store.agentEvent("reasoning", "유닛을 ");
    store.agentEvent("reasoning", "확인합니다.");
    expect(store.getState().turn.reasoning).toBe("먼저 유닛을 확인합니다.");
  });

  it("accumulates delta answer text into turn.answer and marks the answer started", () => {
    const store = readyWithProject();
    store.chatSent();
    expect(store.getState().turn.answerStarted).toBe(false);
    store.agentEvent("delta", "HP를 ");
    store.agentEvent("delta", "80으로 변경했습니다.");
    expect(store.getState().turn.answer).toBe("HP를 80으로 변경했습니다.");
    expect(store.getState().turn.answerStarted).toBe(true);
  });

  it("records tool_call events as tool rows with the tool name", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set unit hp");
    store.agentEvent("tool_call", "file_write main.eps");
    const tools = store.getState().turn.tools;
    expect(tools).toHaveLength(2);
    expect(tools[0].name).toBe("dat_set unit hp");
    expect(tools[1].name).toBe("file_write main.eps");
  });

  it("never pushes a raw kind identifier into the log", () => {
    const store = readyWithProject();
    store.chatSent();
    for (const kind of [
      "delta",
      "reasoning",
      "answer",
      "token_usage",
      "turn_done",
      "item_started",
      "item_completed",
      "event",
      "tool_call",
      "tool_result",
    ]) {
      store.agentEvent(kind, "payload");
    }
    const logText = store
      .getState()
      .log.map((e) => `${e.kind}:${e.text}`)
      .join("\n");
    for (const raw of [
      "delta",
      "token_usage",
      "turn_done",
      "item_started",
      "item_completed",
    ]) {
      expect(logText).not.toContain(raw);
    }
  });

  it("resets the per-turn buffers when a new turn starts (chatSent)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("reasoning", "이전 추론");
    store.agentEvent("delta", "이전 답변");
    store.agentEvent("tool_call", "dat_set");
    // a new turn (the changeset auto-accepts server-side; follow-up chat)
    store.changesetReceived("r1", [
      { category: "file", id: "e1", seq: 0, kind: "created", path: "a.eps" },
    ]);
    store.chatSent();
    const turn = store.getState().turn;
    expect(turn.reasoning).toBe("");
    expect(turn.answer).toBe("");
    expect(turn.answerStarted).toBe(false);
    expect(turn.tools).toEqual([]);
  });

  it("resets the per-turn buffers on plan_feedback / plan_approve", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# 계획", 1);
    // plan_review: feedback starts a fresh turn
    store.agentEvent("reasoning", "leftover");
    store.planFeedbackSent();
    expect(store.getState().turn.reasoning).toBe("");
  });
});

// ---- EUD-068: tool_call args + tool_result text/status ride agent_event.data.
// The server now forwards McpToolCall arguments (item/started) and the result
// text + completion status (item/completed) so the Tool cards can show what was
// requested and what came back (live-E2E defect 2).
describe("agentEvent tool args/result (EUD-068)", () => {
  it("stores tool_call args from the data field", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set", {
      args: '{"dat":"units","objId":0,"param":"Hit Points","value":20480}',
    });
    const t = store.getState().turn.tools[0];
    expect(t.name).toBe("dat_set");
    expect(t.args).toContain("Hit Points");
  });

  it("stores tool_result text and keeps state done on completed", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_get", { args: "{}" });
    store.agentEvent("tool_result", "dat_get", {
      result: "OK: units|Hit Points|0 = 20480",
      status: "completed",
    });
    const t = store.getState().turn.tools[0];
    expect(t.state).toBe("done");
    expect(t.detail).toContain("20480");
  });

  it("flags a failed tool_result", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set", { args: "{}" });
    store.agentEvent("tool_result", "dat_set", {
      result: "ERROR: invalid dat name",
      status: "failed",
    });
    const t = store.getState().turn.tools[0];
    expect(t.state).toBe("failed");
    expect(t.detail).toContain("invalid dat name");
  });

  it("keeps working without a data field (legacy server shape)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "build_run");
    store.agentEvent("tool_result", "build_run");
    const t = store.getState().turn.tools[0];
    expect(t.state).toBe("done");
    expect(t.args).toBeUndefined();
  });
});

// ---- EUD-069: turn-end tool archiving. The live tool rows render INLINE in
// the conversation; when the turn ends they are archived into the log as a
// compact entry CARRYING the tool rows (LogEntry.tools) and the buffer clears —
// stale rows must not occupy the screen into the next phase (the live-E2E
// layout crush: 14 leftover rows squeezed the plan card to 33px).
describe("turn-end tool archiving (EUD-069)", () => {
  function runTools(store: ReturnType<typeof createPanelStore>) {
    store.agentEvent("tool_call", "dat_get", { args: "{}" });
    store.agentEvent("tool_result", "dat_get", {
      result: "OK",
      status: "completed",
    });
    store.agentEvent("tool_call", "dat_set", { args: "{}" });
    store.agentEvent("tool_result", "dat_set", {
      result: "OK",
      status: "completed",
    });
  }

  function archivedEntry(store: ReturnType<typeof createPanelStore>) {
    return store.getState().log.find((e) => e.tools !== undefined);
  }

  it("archives tool rows into the log and clears them on answerReceived", () => {
    const store = readyWithProject();
    store.chatSent();
    runTools(store);
    store.answerReceived("끝");
    expect(store.getState().turn.tools).toEqual([]);
    const entry = archivedEntry(store);
    expect(entry).toBeDefined();
    expect(entry!.text).toContain("도구 호출 2건");
    expect(entry!.tools).toHaveLength(2);
  });

  it("archives tool rows when a plan ends the turn", () => {
    const store = readyWithProject();
    store.chatSent();
    runTools(store);
    store.planReceived("# 계획", 1);
    expect(store.getState().turn.tools).toEqual([]);
    expect(archivedEntry(store)).toBeDefined();
  });

  it("archives tool rows when a changeset ends the turn", () => {
    const store = readyWithProject();
    store.chatSent();
    runTools(store);
    store.changesetReceived("r1", [
      { category: "file", id: "e1", seq: 0, kind: "created", path: "a.eps" },
    ]);
    expect(store.getState().turn.tools).toEqual([]);
    expect(archivedEntry(store)).toBeDefined();
  });

  it("archives tool rows when the turn errors out", () => {
    const store = readyWithProject();
    store.chatSent();
    runTools(store);
    store.errorReceived("agent turn failed: boom");
    expect(store.getState().turn.tools).toEqual([]);
    expect(archivedEntry(store)).toBeDefined();
  });

  it("adds no archive entry when no tools ran", () => {
    const store = readyWithProject();
    store.chatSent();
    store.answerReceived("끝");
    expect(archivedEntry(store)).toBeUndefined();
  });

  it("aggregates repeated tool names in the archive text", () => {
    const store = readyWithProject();
    store.chatSent();
    for (let i = 0; i < 3; i += 1) {
      store.agentEvent("tool_call", "dat_get", { args: "{}" });
      store.agentEvent("tool_result", "dat_get", {
        result: "OK",
        status: "completed",
      });
    }
    store.answerReceived("끝");
    expect(archivedEntry(store)!.text).toContain("dat_get×3");
  });
});

describe("resetSent — new conversation (EUD-064/065)", () => {
  it("clears log, plan, changeset, and the turn buffers, returning to ready", () => {
    // No undecided changeset here, so the fresh log is empty (the discard notice
    // — F3 — is covered separately below).
    const store = readyWithProject();
    store.chatSent();
    store.log("you", "이전 메시지");
    store.planReceived("# 계획", 1);
    store.agentEvent("delta", "답변");
    store.resetSent();
    const s = store.getState();
    expect(s.log).toEqual([]);
    expect(s.plan).toBeNull();
    expect(s.changeset).toBeNull();
    expect(s.turn.answer).toBe("");
    expect(s.turn.reasoning).toBe("");
    expect(s.phase).toBe("ready");
  });

  // F3: resetting over an undecided changeset logs a notice (the server
  // default-accepts the undecided items) as the FIRST entry of the fresh log.
  it("logs a discard notice when an undecided changeset is reset away (F3)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("r1", [
      { category: "file", id: "e1", seq: 0, kind: "created", path: "a.eps" },
    ]);
    store.resetSent();
    const log = store.getState().log;
    expect(log).toHaveLength(1);
    expect(log[0].kind).toBe("warn");
    expect(log[0].text).toContain("자동 적용");
  });

  it("does NOT log a discard notice when no changeset is open (F3)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.resetSent();
    expect(store.getState().log).toEqual([]);
  });

  it("does NOT log a discard notice when the changeset is fully decided (F3)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.changesetReceived("r1", [
      { category: "file", id: "e1", seq: 0, kind: "created", path: "a.eps" },
    ]);
    // Decide the single item, then reset — nothing was left undecided. A
    // per-item accept carries the real id back in the rollback_result reply
    // (only bulk accept echoes an empty ids array).
    store.decisionSent("accept", ["e1"]);
    store.rollbackResult(["e1"], true);
    store.resetSent();
    expect(store.getState().log).toEqual([]);
  });
});

// F2: prose streamed via `delta` before a non-answer turn-end (plan/changeset/
// error) is archived as a prominent agent log entry — otherwise the live
// AgentAnswer bubble's text vanishes at the transition. The answer{} path is
// authoritative and does NOT double-log.
describe("streamed-prose archival on turn-end (F2)", () => {
  function logTexts(store: ReturnType<typeof readyWithProject>): string[] {
    return store
      .getState()
      .log.filter((e) => e.kind === "agent")
      .map((e) => e.text);
  }

  it("archives streamed prose when the turn ends with plan{}", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "먼저 계획을 ");
    store.agentEvent("delta", "세웁니다.");
    store.planReceived("# 계획", 1);
    expect(logTexts(store)).toContain("먼저 계획을 세웁니다.");
    // The buffer is cleared after archiving (no re-archive on a later transition).
    expect(store.getState().turn.answer).toBe("");
  });

  it("archives streamed prose when the turn ends with changeset{}", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "변경을 적용했습니다.");
    store.changesetReceived("r1", [
      { category: "file", id: "e1", seq: 0, kind: "created", path: "a.eps" },
    ]);
    expect(logTexts(store)).toContain("변경을 적용했습니다.");
  });

  it("archives streamed prose when the turn ends with error{}", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "진행 중이던 설명");
    store.errorReceived("boom");
    expect(logTexts(store)).toContain("진행 중이던 설명");
  });

  it("does not archive an empty/whitespace buffer", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "   ");
    store.planReceived("# 계획", 1);
    expect(logTexts(store)).not.toContain("   ");
  });

  it("the answer{} path does NOT double-log (buffer not separately archived)", () => {
    // answerReceived ends the turn but does NOT archive the buffer (the App layer
    // logs the authoritative server text). Simulate the App flow: deltas stream,
    // then answer{} arrives → answerReceived + a single agent log of the final
    // text. The buffer must not produce a second agent entry.
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "부분 답변");
    store.answerReceived("최종 답변"); // App then logs the authoritative text
    store.log("agent", "최종 답변");
    const agents = logTexts(store);
    expect(agents).toEqual(["최종 답변"]);
  });
});

// ---- RAG warmup send gate: the server replays the current rag_warmup state to
// a newly connected client; while the model loads (~19s) sending is blocked so
// a turn does not silently park on the warmup lock. Fail-open everywhere else:
// "unknown" (old server / tests — no snapshot) and "unavailable" (warmup error)
// must NEVER lock the panel.
describe("RAG warmup send gate", () => {
  it("defaults to 'unknown' and does NOT block send (fail-open)", () => {
    const store = readyWithProject();
    expect(store.getState().rag).toBe("unknown");
    expect(store.getState().canSend).toBe(true);
  });

  it("blocks send while the RAG model is loading", () => {
    const store = readyWithProject();
    store.ragWarmupChanged("loading");
    const s = store.getState();
    expect(s.rag).toBe("loading");
    expect(s.canSend).toBe(false);
  });

  it("unblocks send when warmup completes", () => {
    const store = readyWithProject();
    store.ragWarmupChanged("loading");
    store.ragWarmupChanged("ready");
    expect(store.getState().canSend).toBe(true);
  });

  it("unblocks send when warmup fails (fail-open — never lock forever)", () => {
    const store = readyWithProject();
    store.ragWarmupChanged("loading");
    store.ragWarmupChanged("unavailable");
    expect(store.getState().canSend).toBe(true);
  });

  it("keeps the other gates: rag ready does not bypass the project gate", () => {
    const store = freshStore();
    store.wsOpen();
    store.ragWarmupChanged("ready");
    expect(store.getState().canSend).toBe(false); // no project open
  });
});

// Type-only: PanelState carries the v2 fields the (future) UI renders from.
const _typecheck: PanelState = createPanelStore().getState();
void _typecheck;
