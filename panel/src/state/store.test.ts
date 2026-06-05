import { describe, it, expect } from "vitest";
import {
  createPanelStore,
  MAX_LOG_ENTRIES,
  type PanelState,
} from "@/state/store";
import { CLIENT_MESSAGE_TYPES, SERVER_MESSAGE_TYPES } from "@/ws/protocol";

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

  it("blocks send while busy (plan_review awaits feedback/approve, not chat)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    expect(store.getState().canSend).toBe(false);
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

// Type-only: PanelState carries the v2 fields the (future) UI renders from.
const _typecheck: PanelState = createPanelStore().getState();
void _typecheck;
