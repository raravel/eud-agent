/**
 * Conversation / event log (features/03 ## Behaviors → Conversation):
 *   instructions, progress entries (spinner on the ACTIVE stage incl.
 *   waiting_build), errors, applied confirmations — driven by the store's
 *   capped log.
 *
 * Contract (Step B implements `@/components/ConversationLog`):
 *   export interface ConversationLogProps {
 *     log: LogEntry[];     // store log entries (kind/text/stage)
 *     phase: Phase;        // to know which progress entry is "active"
 *   }
 *   export function ConversationLog(props): JSX.Element;
 *
 * A progress LogEntry carries a `stage`. The entry that is the LATEST progress
 * entry while the panel is still busy (working/applying/waiting) shows a
 * spinner (role="status"); resolved entries (ok/error) do not. Each entry's
 * text is rendered.
 */
import { describe, it, expect } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { createPanelStore } from "@/state/store";
import { ConversationLog } from "@/components/ConversationLog";

describe("ConversationLog — entries", () => {
  it("renders an instruction (you) entry's text", () => {
    const store = createPanelStore();
    store.log("you", "트리거를 추가해줘");
    render(<ConversationLog log={store.getState().log} phase="working" />);
    expect(screen.getByText("트리거를 추가해줘")).toBeInTheDocument();
  });

  it("renders an applied confirmation entry", () => {
    const store = createPanelStore();
    store.log("ok", "main.eps에 적용되었습니다.");
    render(<ConversationLog log={store.getState().log} phase="ready" />);
    expect(screen.getByText("main.eps에 적용되었습니다.")).toBeInTheDocument();
  });

  it("renders an error entry", () => {
    const store = createPanelStore();
    store.log("error", "오류: editor busy");
    render(<ConversationLog log={store.getState().log} phase="ready" />);
    expect(screen.getByText("오류: editor busy")).toBeInTheDocument();
  });
});

describe("ConversationLog — progress spinner on the active stage", () => {
  it("shows a spinner on the latest progress entry while working", () => {
    const store = createPanelStore();
    store.log("progress", "RAG 검색 중…", "rag");
    render(<ConversationLog log={store.getState().log} phase="working" />);
    const entry = screen.getByTestId("log-entry-rag");
    expect(within(entry).getByRole("status")).toBeInTheDocument();
  });

  it("shows a spinner on a waiting_build progress entry while waiting", () => {
    const store = createPanelStore();
    store.log("progress", "빌드 대기 중…", "waiting_build");
    render(<ConversationLog log={store.getState().log} phase="waiting" />);
    const entry = screen.getByTestId("log-entry-waiting_build");
    expect(within(entry).getByRole("status")).toBeInTheDocument();
  });

  it("does NOT show a spinner once the flow is back to ready", () => {
    const store = createPanelStore();
    store.log("progress", "codex 실행 중…", "codex");
    render(<ConversationLog log={store.getState().log} phase="ready" />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("spins only the LATEST progress entry, not earlier ones", () => {
    const store = createPanelStore();
    store.log("progress", "RAG 검색 중…", "rag");
    store.log("progress", "codex 실행 중…", "codex");
    render(<ConversationLog log={store.getState().log} phase="working" />);
    expect(
      within(screen.getByTestId("log-entry-codex")).queryByRole("status"),
    ).toBeInTheDocument();
    expect(
      within(screen.getByTestId("log-entry-rag")).queryByRole("status"),
    ).not.toBeInTheDocument();
  });
});
