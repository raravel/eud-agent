import { describe, it, expect } from "vitest";
import {
  isServerMessage,
  isProgressMessage,
  isAgentEventMessage,
  isAnswerMessage,
  isPlanMessage,
  isChangesetMessage,
  isRollbackResultMessage,
  isErrorMessage,
  isStatusMessage,
  isListMessage,
  CLIENT_MESSAGE_TYPES,
  SERVER_MESSAGE_TYPES,
  type ClientMessage,
  type ServerMessage,
} from "@/ws/protocol";

describe("server message type guards (v2)", () => {
  it("accepts a well-formed progress message (stage is an open string)", () => {
    const msg = { type: "progress", stage: "rag", detail: "searching" };
    expect(isServerMessage(msg)).toBe(true);
    expect(isProgressMessage(msg)).toBe(true);
  });

  it("accepts rag_warmup and other stages (open string)", () => {
    for (const stage of ["rag", "rag_warmup", "codex", "lsp", "waiting_build"]) {
      expect(isProgressMessage({ type: "progress", stage })).toBe(true);
    }
  });

  it("accepts an agent_event (kind + detail)", () => {
    const msg = { type: "agent_event", kind: "tool_call", detail: "dat_set" };
    expect(isAgentEventMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
  });

  it("accepts an answer message", () => {
    expect(isAnswerMessage({ type: "answer", text: "hello" })).toBe(true);
  });

  it("accepts a plan message (markdown + numeric revision)", () => {
    const msg = { type: "plan", markdown: "# plan", revision: 1 };
    expect(isPlanMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
    // a non-numeric revision is rejected
    expect(isPlanMessage({ type: "plan", markdown: "x", revision: "1" })).toBe(
      false,
    );
  });

  it("accepts a changeset message (request_id + items array)", () => {
    const msg = {
      type: "changeset",
      request_id: "req-1",
      items: [{ category: "file", id: "e1", seq: 0, kind: "created" }],
    };
    expect(isChangesetMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
  });

  it("accepts a rollback_result message (ids + ok)", () => {
    const msg = { type: "rollback_result", ids: ["e1"], ok: true };
    expect(isRollbackResultMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
  });

  it("accepts error / status / list messages", () => {
    expect(isErrorMessage({ type: "error", message: "boom" })).toBe(true);
    expect(
      isStatusMessage({ type: "status", compiling: false, project: "p" }),
    ).toBe(true);
    expect(
      isListMessage({
        type: "list",
        files: [{ path: "a.eps", ftype: "CUIEps", settable: true }],
      }),
    ).toBe(true);
  });

  it("rejects unknown message types (not a server message)", () => {
    expect(isServerMessage({ type: "mystery" })).toBe(false);
    expect(isProgressMessage({ type: "agent_event" })).toBe(false);
  });

  it("rejects non-objects and missing type", () => {
    expect(isServerMessage(null)).toBe(false);
    expect(isServerMessage(undefined)).toBe(false);
    expect(isServerMessage("string")).toBe(false);
    expect(isServerMessage(42)).toBe(false);
    expect(isServerMessage({})).toBe(false);
  });
});

describe("v1 message types are removed entirely (no compat shim)", () => {
  it("v1 server types (code/applied) are not in the discriminant set", () => {
    expect(SERVER_MESSAGE_TYPES).not.toContain("code");
    expect(SERVER_MESSAGE_TYPES).not.toContain("applied");
  });

  it("v1 client types (instruct/apply) are not in the discriminant set", () => {
    expect(CLIENT_MESSAGE_TYPES).not.toContain("instruct");
    expect(CLIENT_MESSAGE_TYPES).not.toContain("apply");
  });

  it("a v1 code/applied frame is not a valid server message", () => {
    expect(
      isServerMessage({
        type: "code",
        code: "c",
        lang: "eps",
        diff: "",
        diagnostics: [],
      }),
    ).toBe(false);
    expect(isServerMessage({ type: "applied", target: "main.eps" })).toBe(false);
  });
});

describe("client message shapes (compile-time + structural)", () => {
  it("constructs every documented v2 client message", () => {
    const chat: ClientMessage = { type: "chat", text: "do it" };
    const planFeedback: ClientMessage = {
      type: "plan_feedback",
      text: "tweak it",
    };
    const planApprove: ClientMessage = { type: "plan_approve" };
    const decisionAll: ClientMessage = {
      type: "changeset_decision",
      decision: "accept",
      ids: "all",
    };
    const decisionSome: ClientMessage = {
      type: "changeset_decision",
      decision: "reject",
      ids: ["e1", "e2"],
    };
    const cancel: ClientMessage = { type: "cancel" };
    const status: ClientMessage = { type: "status" };
    const list: ClientMessage = { type: "list" };
    expect(chat.text).toBe("do it");
    expect(planFeedback.type).toBe("plan_feedback");
    expect(planApprove.type).toBe("plan_approve");
    expect(decisionAll.ids).toBe("all");
    expect(decisionSome.decision).toBe("reject");
    expect(cancel.type).toBe("cancel");
    expect(status.type).toBe("status");
    expect(list.type).toBe("list");
  });
});

describe("server message discriminated union (v2)", () => {
  it("narrows on type", () => {
    const msgs: ServerMessage[] = [
      { type: "agent_event", kind: "thinking", detail: "" },
      { type: "answer", text: "a" },
      { type: "plan", markdown: "p", revision: 1 },
      { type: "changeset", request_id: "r", items: [] },
      { type: "rollback_result", ids: [], ok: true },
      { type: "progress", stage: "codex" },
      { type: "error", message: "m" },
      { type: "status", compiling: true, project: "p" },
      { type: "list", files: [] },
    ];
    expect(msgs.map((m) => m.type)).toEqual([
      "agent_event",
      "answer",
      "plan",
      "changeset",
      "rollback_result",
      "progress",
      "error",
      "status",
      "list",
    ]);
  });
});
