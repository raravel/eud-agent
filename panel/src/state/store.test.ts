import { describe, it, expect } from "vitest";
import {
  createPanelStore,
  MAX_LOG_ENTRIES,
  type PanelState,
} from "@/state/store";
import {
  CLIENT_MESSAGE_TYPES,
  SERVER_MESSAGE_TYPES,
  type MentionInstance,
} from "@/lib/ipc";

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

const regionMention = {
  id: "mention-region",
  label: "영역 A",
  detail: "저장된 영역 · 사각형",
  mention: {
    kind: "map.region",
    version: 1,
    projectId: "project-a",
    sourceFileSha256: "a".repeat(64),
    mapWidth: 64,
    mapHeight: 64,
    selectionId: "region-a",
    selectionSnapshotHash: "b".repeat(64),
  },
} satisfies MentionInstance;

describe("initial state", () => {
  it("starts in connecting with an empty log, no project, no plan", () => {
    const s = freshStore().getState();
    expect(s.phase).toBe("connecting");
    expect(s.log).toEqual([]);
    expect(s.hasProject).toBe(false);
    expect(s.files).toEqual([]);
    expect(s.plan).toBeNull();
    expect(s.ask).toBeNull();
    expect(s.contextUsage).toBeNull();
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

describe("turn transitions (ready <-> thinking -> plan_review)", () => {
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

  it("records the turn's commit quietly and announces an external one", () => {
    const store = readyWithProject();
    store.chatSent();
    store.gitCommitted({
      external: { sha: "aaa1111", subject: "앱 밖 변경", files: 2 },
      turn: { sha: "bbb2222", subject: "마린 체력 조정", files: 1 },
    });
    const log = store.getState().log;
    const outside = log.find((entry) => entry.text.includes("앱 밖에서 바뀐 파일"));
    const turn = log.find((entry) => entry.text.includes("마린 체력 조정"));
    // The external commit is the surprising one, so it is neither muted nor
    // suppressed; the turn's own commit is a quiet, toast-free row.
    expect(outside?.kind).toBe("ok");
    expect(outside?.text).toContain("2개");
    expect(outside?.silent).toBeUndefined();
    expect(turn?.kind).toBe("info");
    expect(turn?.silent).toBe(true);
    // Recording is bookkeeping: it never moves the turn out of thinking.
    expect(store.getState().phase).toBe("thinking");
  });

  it("reports a failed record as a warning without losing the turn", () => {
    const store = readyWithProject();
    store.chatSent();
    store.gitCommitted({ warning: "이 턴을 기록하지 못했습니다: 잠김" });
    const warn = store.getState().log.find((entry) => entry.kind === "warn");
    expect(warn?.text).toBe("이 턴을 기록하지 못했습니다: 잠김");
    expect(store.getState().phase).toBe("thinking");
  });
});

describe("ASK lifecycle", () => {
  const questions = [
    {
      id: "mode",
      question: "방식을 고르세요.",
      multi: false,
      options: [{ label: "A" }, { label: "B" }],
    },
  ];

  it("keeps the same turn active while the user answers", () => {
    const store = readyWithProject();
    store.chatSent();
    store.askReceived("ask-1", questions);

    expect(store.getState().phase).toBe("thinking");
    expect(store.getState().ask).toEqual({
      requestId: "ask-1",
      questions,
      submitting: false,
    });

    store.askSubmitStarted();
    expect(store.getState().ask?.submitting).toBe(true);
    store.askSubmitFailed();
    expect(store.getState().ask?.submitting).toBe(false);
    store.askAnswered();

    expect(store.getState().ask).toBeNull();
    expect(store.getState().phase).toBe("thinking");
  });

  it("restores a backend-pending ASK after local turn state was lost", () => {
    const store = readyWithProject();

    store.askReceived("ask-recovered", questions);

    expect(store.getState().phase).toBe("thinking");
    expect(store.getState().ask).toEqual({
      requestId: "ask-recovered",
      questions,
      submitting: false,
    });
  });

  it("closes an expired question and tells the user the answer continues as text", () => {
    const store = readyWithProject();
    store.chatSent();
    store.askReceived("ask-3", questions, 240);
    expect(store.getState().ask?.waitSeconds).toBe(240);
    expect(store.getState().ask?.receivedAt).toBeTypeOf("number");

    store.askExpired("ask-other");
    expect(store.getState().ask?.requestId).toBe("ask-3");

    store.askExpired("ask-3");
    expect(store.getState().ask).toBeNull();
    expect(store.getState().phase).toBe("thinking");
    const last = store.getState().log.at(-1)!;
    expect(last.kind).toBe("info");
    expect(last.text).toContain("240초");
  });

  it("clears a pending question when the turn is cancelled", () => {
    const store = readyWithProject();
    store.chatSent();
    store.askReceived("ask-2", questions);
    store.cancelSent();

    expect(store.getState().ask).toBeNull();
    expect(store.getState().phase).toBe("ready");
  });
});

describe("cancel and message rewind", () => {
  it("archives the partial answer and ignores events that arrive after cancel", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "여기까지 처리");

    store.cancelSent();
    store.agentEvent("delta", "이 텍스트는 늦게 도착");

    expect(store.getState().phase).toBe("ready");
    expect(store.getState().log.map((entry) => entry.text)).toEqual([
      "여기까지 처리",
      "작업을 중단했습니다.",
    ]);
  });

  it("removes the selected user message and every later row", () => {
    const store = readyWithProject();
    store.log("you", "첫 요청");
    store.log("agent", "첫 답변");
    store.log("you", "수정할 요청", undefined, undefined, [regionMention]);
    const selectedId = store.getState().log.at(-1)!.id;
    store.log("agent", "제거할 답변");

    const restored = store.rewindTo(selectedId);

    expect(restored?.text).toBe("수정할 요청");
    expect(restored?.mentions).toEqual([regionMention]);
    expect(store.getState().log.map((entry) => entry.text)).toEqual([
      "첫 요청",
      "첫 답변",
    ]);
    expect(store.getState().phase).toBe("ready");
    expect(store.getState().turn).toEqual({
      reasoning: "",
      answer: "",
      answerStarted: false,
      tools: [],
      blocks: [],
    });
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

  it("a wsOpen mid-plan_review keeps the plan under review (durable review state)", () => {
    const store = readyWithProject();
    store.chatSent();
    store.planReceived("# plan", 1);
    store.wsConnecting();
    store.wsOpen();
    const s = store.getState();
    expect(s.phase).toBe("plan_review");
    expect(s.plan).toEqual({ markdown: "# plan", revision: 1 });
    // No turn was in flight (plan_review awaits a decision): no cancel notice.
    expect(s.log.some((entry) => entry.kind === "warn")).toBe(false);
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

  it("keeps stable client turn anchors aligned after the 500-entry cap", () => {
    const store = freshStore();
    for (let i = 0; i < MAX_LOG_ENTRIES + 1; i += 1) {
      store.log(
        "you",
        `turn ${i}`,
        undefined,
        undefined,
        undefined,
        `client-turn-${i}`,
      );
    }
    const log = store.getState().log;
    expect(log[0].clientTurnId).toBe("client-turn-1");
    expect(log.at(-1)?.clientTurnId).toBe(`client-turn-${MAX_LOG_ENTRIES}`);
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
  it("client message types exclude removed instruct/apply/reset commands", () => {
    expect(CLIENT_MESSAGE_TYPES).not.toContain("instruct");
    expect(CLIENT_MESSAGE_TYPES).not.toContain("apply");
    expect(CLIENT_MESSAGE_TYPES).not.toContain("reset");
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
        "ask_response",
        "cancel",
        "conversation_rewind",
        "status",
        "list",
      ]),
    );
    expect(SERVER_MESSAGE_TYPES).toEqual(
      expect.arrayContaining([
        "agent_event",
        "context_usage",
        "answer",
        "plan",
        "ask",
        "git",
        "error",
        "status",
        "progress",
        "list",
      ]),
    );
  });
});

// ---- per-session Context usage ---------------------------------------

describe("per-session context usage", () => {
  const usage = {
    last: {
      inputTokens: 31_000,
      cachedInputTokens: 24_000,
      cacheWriteInputTokens: 0,
      outputTokens: 1_200,
      reasoningOutputTokens: 800,
      totalTokens: 32_200,
    },
    total: {
      inputTokens: 52_000,
      cachedInputTokens: 40_000,
      cacheWriteInputTokens: 600,
      outputTokens: 2_100,
      reasoningOutputTokens: 1_300,
      totalTokens: 54_100,
    },
    modelContextWindow: 128_000,
  };

  it("replaces the latest snapshot independently from turn state", () => {
    const store = freshStore();
    store.contextUsageReceived(usage);

    expect(store.getState().contextUsage).toEqual(usage);
    expect(store.getState().phase).toBe("connecting");
  });

  it("clears stale usage when rewinding to a fresh Codex thread", () => {
    const store = readyWithProject();
    store.log("you", "다시 작성할 요청");
    const entry = store.getState().log[0];
    store.contextUsageReceived(usage);

    store.rewindTo(entry.id);

    expect(store.getState().contextUsage).toBeNull();
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
    store.agentEvent("tool_call", "dat_set unit hp", { callId: "call-dat" });
    store.agentEvent("tool_call", "file_write main.eps", {
      callId: "call-file",
    });
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
    // a new turn
    store.answerReceived("끝");
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
  it("matches interleaved same-name tool results by call id", () => {
    // Given: two overlapping calls with the same display name and distinct IDs.
    const store = readyWithProject();
    store.chatSent();
    const firstCall = { callId: "call-a", args: '{"path":"a.eps"}' };
    const secondCall = { callId: "call-b", args: '{"path":"b.eps"}' };
    store.agentEvent("tool_call", "read_file", firstCall);
    store.agentEvent("tool_call", "read_file", secondCall);

    // When: their results arrive in start order rather than stack order.
    store.agentEvent("tool_result", "read_file", {
      callId: "call-a",
      result: "alpha",
      status: "completed",
    });
    store.agentEvent("tool_result", "read_file", {
      callId: "call-b",
      result: "beta-error",
      status: "failed",
    });

    // Then: each result and terminal state stays with its originating call.
    expect(store.getState().turn.tools).toMatchObject([
      {
        callId: "call-a",
        args: '{"path":"a.eps"}',
        detail: "alpha",
        state: "done",
      },
      {
        callId: "call-b",
        args: '{"path":"b.eps"}',
        detail: "beta-error",
        state: "failed",
      },
    ]);
  });

  it("keeps unknown and duplicate keyed results from changing another call", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "read_file", {
      callId: "known",
      args: '{"path":"known.eps"}',
    });
    store.agentEvent("tool_result", "read_file", {
      callId: "unknown",
      result: "orphan-error",
      status: "failed",
    });
    store.agentEvent("tool_result", "read_file", {
      callId: "known",
      result: "known-result",
      status: "completed",
    });
    store.agentEvent("tool_result", "read_file", {
      callId: "known",
      result: "duplicate-error",
      status: "failed",
    });

    expect(store.getState().turn.tools).toMatchObject([
      { callId: "known", state: "done", detail: "known-result" },
      { callId: "unknown", state: "failed", detail: "orphan-error" },
    ]);
  });

  it("renders id-less starts as information and id-less terminals as their own row", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "command", {
      callId: "",
      args: "cargo test",
    });

    expect(store.getState().turn.tools).toEqual([]);
    expect(store.getState().log.at(-1)).toMatchObject({
      kind: "info",
      text: "도구 호출 시작 — command\ncargo test",
    });

    store.agentEvent("tool_result", "command", {
      result: "12 passed",
      status: "completed",
    });
    expect(store.getState().turn.tools).toMatchObject([
      { name: "command", state: "done", detail: "12 passed" },
    ]);
  });

  it("stores tool_call args from the data field", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set", {
      callId: "call-dat-set",
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

  it("preserves supported id-less native observations", () => {
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


// F2: prose streamed via `delta` before a non-answer turn-end (plan/error)
// is archived as a prominent agent log entry — otherwise the live
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

  it("the answer{} path does NOT double-log (streamed prose IS the answer)", () => {
    // answerReceived archives the streamed blocks itself (the App no longer
    // logs msg.text — the final answer{} text is the same deltas concatenated,
    // so logging it again would duplicate the prose). The final text is used
    // only when nothing streamed.
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("delta", "부분 답변");
    store.answerReceived("최종 답변");
    expect(logTexts(store)).toEqual(["부분 답변"]);
  });

  it("answerReceived logs the final text when no prose was streamed", () => {
    const store = readyWithProject();
    store.chatSent();
    store.answerReceived("최종 답변");
    expect(logTexts(store)).toEqual(["최종 답변"]);
  });
});

// ---- Turn activity blocks: the codex turn is a SEQUENCE of items (message →
// tool calls → message → …). The blocks timeline keeps arrival order so the
// live surface and the archived history interleave tools and prose
// chronologically (regression: all tool rows rendered above all prose), and an
// item boundary (item_started / a tool call) starts a NEW text block so
// separate messages don't glue into one paragraph.
describe("turn activity blocks (chronological interleave)", () => {
  function blockShape(store: ReturnType<typeof readyWithProject>) {
    return store.getState().turn.blocks.map((b) =>
      b.type === "text" ? `text:${b.text}` : `tools:${b.tools.map((t) => t.name).join(",")}`,
    );
  }

  it("interleaves text and tools blocks in arrival order", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("item_started", "item_1");
    store.agentEvent("delta", "먼저 ");
    store.agentEvent("delta", "확인합니다.");
    store.agentEvent("tool_call", "search_docs", { args: "{}" });
    store.agentEvent("tool_result", "search_docs", {
      result: "2 hits",
      status: "completed",
    });
    store.agentEvent("item_started", "item_3");
    store.agentEvent("delta", "결과를 적용했습니다.");
    expect(blockShape(store)).toEqual([
      "text:먼저 확인합니다.",
      "tools:search_docs",
      "text:결과를 적용했습니다.",
    ]);
  });

  it("starts a new text block per message item even without tools between", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("item_started", "item_1");
    store.agentEvent("delta", "첫 번째 메시지.");
    store.agentEvent("item_started", "item_2");
    store.agentEvent("delta", "두 번째 메시지.");
    expect(blockShape(store)).toEqual([
      "text:첫 번째 메시지.",
      "text:두 번째 메시지.",
    ]);
  });

  it("flips the tool state inside the blocks timeline on tool_result", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set", { args: "{}" });
    store.agentEvent("tool_result", "dat_set", {
      result: "ERROR",
      status: "failed",
    });
    const block = store.getState().turn.blocks[0];
    if (block.type !== "tools") throw new Error("expected a tools block");
    expect(block.tools[0].state).toBe("failed");
    expect(block.tools[0].detail).toBe("ERROR");
  });

  it("archives the blocks in chronological order at turn end", () => {
    const store = readyWithProject();
    store.chatSent();
    store.agentEvent("item_started", "item_1");
    store.agentEvent("delta", "확인 중입니다.");
    store.agentEvent("tool_call", "search_docs", { args: "{}" });
    store.agentEvent("tool_result", "search_docs", {
      result: "OK",
      status: "completed",
    });
    store.agentEvent("item_started", "item_3");
    store.agentEvent("delta", "적용했습니다.");
    store.answerReceived("확인 중입니다.\n\n적용했습니다.");
    const entries = store
      .getState()
      .log.filter((e) => e.kind === "agent" || e.tools !== undefined)
      .map((e) => (e.tools !== undefined ? `tools:${e.text}` : `agent:${e.text}`));
    expect(entries).toEqual([
      "agent:확인 중입니다.",
      "tools:도구 호출 1건 — search_docs",
      "agent:적용했습니다.",
    ]);
    expect(store.getState().turn.blocks).toEqual([]);
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

describe("saved attachment log hydration", () => {
  it("restores attachment metadata and keeps only generated image data URLs", () => {
    const store = freshStore();
    store.hydrate({
      schemaVersion: 2,
      logSeq: 1,
      log: [
        {
          id: 1,
          kind: "you",
          text: "",
          attachments: [
            {
              id: "safe-image",
              name: "screen.png",
              mime: "image/png",
              kind: "image",
              size: 10,
              previewUrl: "data:image/png;base64,safe",
            },
            {
              id: "unsafe-preview",
              name: "notes.txt",
              mime: "text/plain",
              kind: "text",
              size: 5,
              previewUrl: "https://example.test/tracker.png",
            },
            {
              id: "audio-preview",
              name: "theme.ogg",
              mime: "audio/ogg",
              kind: "audio",
              size: 4096,
              previewUrl: "data:image/png;base64,not-audio-log-data",
            },
          ],
        },
      ],
    });

    const restored = store.getState().log[0].attachments;
    expect(restored?.[0].previewUrl).toBe("data:image/png;base64,safe");
    expect(restored?.[1].previewUrl).toBeUndefined();
    expect(restored?.[2].previewUrl).toBeUndefined();
  });
  it("hydrates generic mentions and returns them unchanged from rewind", () => {
    const store = freshStore();
    store.hydrate({
      schemaVersion: 2,
      logSeq: 1,
      log: [
        {
          id: 1,
          kind: "you",
          text: "수정할 요청",
          clientTurnId: "11111111-1111-4111-8111-111111111111",
          mentions: [regionMention],
        },
      ],
    });

    expect(store.getState().log[0].mentions).toEqual([regionMention]);
    expect(store.getState().log[0].clientTurnId).toBe(
      "11111111-1111-4111-8111-111111111111",
    );
    const rewound = store.rewindTo(1);
    expect(rewound?.mentions).toEqual([regionMention]);
    expect(rewound?.clientTurnId).toBe(
      "11111111-1111-4111-8111-111111111111",
    );
  });

  it("hydrates a legacy user log without a client turn id", () => {
    const store = freshStore();
    store.hydrate({
      schemaVersion: 2,
      logSeq: 1,
      log: [{ id: 1, kind: "you", text: "legacy" }],
    });
    expect(store.getState().log[0].clientTurnId).toBeUndefined();
    expect(store.rewindTo(1)?.text).toBe("legacy");
  });
});

describe("saved progress log hydration", () => {
  it("drops transient progress rows while preserving the saved id high-water mark", () => {
    const store = freshStore();
    store.hydrate({
      schemaVersion: 2,
      logSeq: 3,
      log: [
        { id: 1, kind: "you", text: "hi" },
        {
          id: 2,
          kind: "progress",
          text: "AI 제공자 실행 중…",
          stage: "provider",
        },
        {
          id: 3,
          kind: "warn",
          text: "구조화된 활성 작업 상태를 갱신하지 못했습니다.",
          stage: "task_state_warning",
        },
      ],
    });

    expect(store.getState().log.map((entry) => entry.id)).toEqual([1, 3]);
    store.log("info", "next");
    expect(store.getState().log.at(-1)?.id).toBe(4);
  });
});

// Type-only: PanelState carries the v2 fields the (future) UI renders from.
const _typecheck: PanelState = createPanelStore().getState();
void _typecheck;
