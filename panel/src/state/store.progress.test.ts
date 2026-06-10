/**
 * Live progress rows are transient turn indicators, not history.
 *
 * "codex 실행 중…" (kind "progress") lines logged during a turn must disappear
 * when the turn ends (answer / plan / changeset / error / cancel) — leaving
 * them reads as a still-running stage after the answer already arrived.
 * Completion records (kind ok/warn, e.g. "RAG 모델 준비 완료") are kept.
 */
import { describe, it, expect } from "vitest";
import { createPanelStore } from "@/state/store";

function logTexts(store: ReturnType<typeof createPanelStore>): string[] {
  return store.getState().log.map((entry) => entry.text);
}

describe("live progress rows clear when the turn ends", () => {
  it("answerReceived drops progress rows but keeps info/ok/user/agent rows", () => {
    const store = createPanelStore();
    store.log("info", "IPC client ready.");
    store.log("you", "안녕?");
    store.chatSent();
    store.log("progress", "codex 실행 중…", "codex");
    store.log("ok", "RAG 모델 준비 완료", "rag_warmup");

    store.answerReceived("안녕하세요!");
    store.log("agent", "안녕하세요!");

    expect(logTexts(store)).toEqual([
      "IPC client ready.",
      "안녕?",
      "RAG 모델 준비 완료",
      "안녕하세요!",
    ]);
    expect(store.getState().phase).toBe("ready");
  });

  it("plan/changeset turn-ends clear the in-flight progress rows too", () => {
    const store = createPanelStore();
    store.chatSent();
    store.log("progress", "RAG 컨텍스트 검색 중…", "rag");
    store.planReceived("# plan", 1);
    expect(logTexts(store)).toEqual([]);

    store.planApproveSent();
    store.log("progress", "codex 실행 중…", "codex");
    store.changesetReceived("req-1", [
      { category: "file", id: "a", seq: 1 },
    ]);
    expect(logTexts(store)).toEqual([]);
  });

  it("errorReceived and cancelSent clear the in-flight progress rows", () => {
    const store = createPanelStore();
    store.chatSent();
    store.log("progress", "codex 실행 중…", "codex");
    store.errorReceived("boom");
    expect(logTexts(store)).toEqual([]);

    store.chatSent();
    store.log("progress", "codex 실행 중…", "codex");
    store.cancelSent();
    expect(logTexts(store)).toEqual([]);
  });
});
