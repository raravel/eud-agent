import { describe, it, expect } from "vitest";
import {
  isServerMessage,
  isProgressMessage,
  isCodeMessage,
  isAppliedMessage,
  isErrorMessage,
  isStatusMessage,
  isListMessage,
  type ClientMessage,
  type ServerMessage,
} from "@/ws/protocol";

describe("server message type guards", () => {
  it("accepts a well-formed progress message", () => {
    const msg = { type: "progress", stage: "rag", detail: "searching" };
    expect(isServerMessage(msg)).toBe(true);
    expect(isProgressMessage(msg)).toBe(true);
  });

  it("accepts every documented progress stage", () => {
    for (const stage of ["rag", "rag_warmup", "codex", "lsp", "waiting_build"]) {
      expect(isProgressMessage({ type: "progress", stage })).toBe(true);
    }
  });

  it("accepts a code message with diff + diagnostics", () => {
    const msg = {
      type: "code",
      code: "function f() {}",
      lang: "eps",
      diff: "--- a\n+++ b\n",
      diagnostics: [],
    };
    expect(isCodeMessage(msg)).toBe(true);
    expect(isServerMessage(msg)).toBe(true);
  });

  it("accepts applied / error / status / list messages", () => {
    expect(isAppliedMessage({ type: "applied", target: "main.eps" })).toBe(true);
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
    expect(isProgressMessage({ type: "code" })).toBe(false);
  });

  it("rejects non-objects and missing type", () => {
    expect(isServerMessage(null)).toBe(false);
    expect(isServerMessage(undefined)).toBe(false);
    expect(isServerMessage("string")).toBe(false);
    expect(isServerMessage(42)).toBe(false);
    expect(isServerMessage({})).toBe(false);
  });
});

describe("client message shapes (compile-time + structural)", () => {
  it("constructs every documented client message", () => {
    const instruct: ClientMessage = {
      type: "instruct",
      instruction: "do it",
      target: "main.eps",
      useContext: true,
    };
    const applySet: ClientMessage = {
      type: "apply",
      mode: "set",
      target: "main.eps",
      code: "x",
    };
    const applyNew: ClientMessage = {
      type: "apply",
      mode: "neweps",
      target: "new.eps",
      code: "x",
    };
    const status: ClientMessage = { type: "status" };
    const list: ClientMessage = { type: "list" };
    expect(instruct.type).toBe("instruct");
    expect(applySet.mode).toBe("set");
    expect(applyNew.mode).toBe("neweps");
    expect(status.type).toBe("status");
    expect(list.type).toBe("list");
  });
});

describe("server message discriminated union", () => {
  it("narrows on type", () => {
    const msgs: ServerMessage[] = [
      { type: "progress", stage: "codex" },
      { type: "code", code: "c", lang: "eps", diff: "", diagnostics: [] },
      { type: "applied", target: "t" },
      { type: "error", message: "m" },
      { type: "status", compiling: true, project: "p" },
      { type: "list", files: [] },
    ];
    expect(msgs.map((m) => m.type)).toEqual([
      "progress",
      "code",
      "applied",
      "error",
      "status",
      "list",
    ]);
  });
});
