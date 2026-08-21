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
import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createPanelStore } from "@/state/store";
import { ConversationLog } from "@/components/ConversationLog";

describe("ConversationLog — entries", () => {
  it("renders an instruction (you) entry's text", () => {
    const store = createPanelStore();
    store.log("you", "트리거를 추가해줘");
    render(<ConversationLog log={store.getState().log} phase="thinking" />);
    expect(screen.getByText("트리거를 추가해줘")).toBeInTheDocument();
  });

  it("renders persisted image previews and file chips on a user message", () => {
    const store = createPanelStore();
    store.log("you", "", undefined, [
      {
        id: "image-1",
        name: "screen.png",
        mime: "image/png",
        kind: "image",
        size: 2048,
        previewUrl: "data:image/png;base64,iVBORw0KGgo=",
      },
      {
        id: "text-1",
        name: "notes.eps",
        mime: "text/plain",
        kind: "text",
        size: 12,
      },
    ]);

    render(<ConversationLog log={store.getState().log} phase="thinking" />);

    expect(
      screen.getByRole("img", { name: "첨부 이미지: screen.png" }),
    ).toBeInTheDocument();
    expect(screen.getByText("notes.eps")).toBeInTheDocument();
    expect(screen.getByText("2 KB")).toBeInTheDocument();
  });

  it("renders Mermaid diagrams in archived agent answers", async () => {
    const store = createPanelStore();
    store.log(
      "agent",
      "```mermaid\nflowchart LR\nA[요청] --> B[완료]\n```",
    );
    const { container } = render(
      <ConversationLog log={store.getState().log} phase="ready" />,
    );

    await waitFor(() => {
      expect(container.querySelector('[data-streamdown="mermaid"]')).not.toBeNull();
    });
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

  it("offers an edit action for user messages", () => {
    const store = createPanelStore();
    store.log("you", "수정할 요청");
    const onEditMessage = vi.fn();
    render(
      <ConversationLog
        log={store.getState().log}
        phase="ready"
        onEditMessage={onEditMessage}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "메시지 수정" }));
    expect(onEditMessage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "you", text: "수정할 요청" }),
    );
  });
});

describe("ConversationLog — progress spinner on the active stage", () => {
  it("shows a spinner on the latest progress entry while working", () => {
    const store = createPanelStore();
    store.log("progress", "RAG 검색 중…", "rag");
    render(<ConversationLog log={store.getState().log} phase="thinking" />);
    const entry = screen.getByTestId("log-entry-rag");
    expect(within(entry).getByRole("status")).toBeInTheDocument();
  });

  it("shows a spinner on a waiting_build progress entry while busy", () => {
    const store = createPanelStore();
    store.log("progress", "빌드 대기 중…", "waiting_build");
    render(<ConversationLog log={store.getState().log} phase="thinking" />);
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
    render(<ConversationLog log={store.getState().log} phase="thinking" />);
    expect(
      within(screen.getByTestId("log-entry-codex")).queryByRole("status"),
    ).toBeInTheDocument();
    expect(
      within(screen.getByTestId("log-entry-rag")).queryByRole("status"),
    ).not.toBeInTheDocument();
  });
});

// ---- EUD-069: the live agent stream renders INLINE inside the conversation
// scroll area (no fixed band between the log and the input — the live-E2E
// layout crush: an unbounded AgentStream band squeezed the log to 0px and the
// plan card to 33px). ConversationLog receives the per-turn buffers and renders
// the Reasoning/Tool surfaces + the live answer bubble INSIDE [role="log"];
// archived tool entries (LogEntry.tools) render as expandable Tool cards.
describe("ConversationLog — inline agent stream (EUD-069)", () => {
  const liveTurn = {
    reasoning: "유닛을 확인 중",
    answer: "부분 답변입니다",
    answerStarted: true,
    tools: [{ id: "t1", name: "dat_get", state: "running" as const }],
    blocks: [],
  };

  it("renders the live turn activity INSIDE the scrollable conversation", () => {
    const store = createPanelStore();
    store.log("you", "마린 체력 2배");
    const { container } = render(
      <ConversationLog
        log={store.getState().log}
        phase="thinking"
        turn={liveTurn}
      />,
    );
    const log = container.querySelector('[role="log"]');
    expect(log).not.toBeNull();
    // The agent activity block is INSIDE the conversation scroll container.
    expect(log!.querySelector('[aria-label="에이전트 활동"]')).not.toBeNull();
    // The live streamed answer bubble is inside too.
    expect(within(log as HTMLElement).getByText("부분 답변입니다")).toBeInTheDocument();
  });

  it("renders the live answer bubble only while thinking", () => {
    const { container } = render(
      <ConversationLog log={[]} phase="ready" turn={liveTurn} />,
    );
    expect(container.textContent).not.toContain("부분 답변입니다");
  });

  it("renders the live turn blocks in chronological order (text/tools interleaved)", () => {
    const store = createPanelStore();
    store.chatSent();
    store.agentEvent("item_started", "item_1");
    store.agentEvent("delta", "먼저 확인합니다.");
    store.agentEvent("tool_call", "search_docs", { args: "{}" });
    store.agentEvent("item_started", "item_3");
    store.agentEvent("delta", "적용했습니다.");
    const { container } = render(
      <ConversationLog
        log={store.getState().log}
        phase="thinking"
        turn={store.getState().turn}
      />,
    );
    // DOM order: first prose bubble → tool group → second prose bubble.
    const text = container.textContent ?? "";
    const first = text.indexOf("먼저 확인합니다.");
    const tools = text.indexOf("도구 호출 1건");
    const second = text.indexOf("적용했습니다.");
    expect(first).toBeGreaterThanOrEqual(0);
    expect(tools).toBeGreaterThan(first);
    expect(second).toBeGreaterThan(tools);
  });

  it("renders an archived tools entry as expandable Tool cards", () => {
    const store = createPanelStore();
    store.chatSent();
    store.agentEvent("tool_call", "dat_set", { args: "{}" });
    store.agentEvent("tool_result", "dat_set", {
      result: "OK",
      status: "completed",
    });
    store.answerReceived("끝");
    render(<ConversationLog log={store.getState().log} phase="ready" />);
    expect(screen.getByText(/도구 호출 1건/)).toBeInTheDocument();
    // The archived row keeps its Tool card (name + state badge).
    expect(screen.getByText("dat_set")).toBeInTheDocument();
    expect(screen.getByText("완료")).toBeInTheDocument();
  });
});


// ---- Empty-conversation hero: an empty log in the ready phase shows guidance
// + clickable example chips (the same chat path as the InstructionBox). Any
// log entry, RAG warmup, or non-ready phase replaces it.
describe("ConversationLog — empty-conversation hero", () => {
  it("shows the hero with suggestion chips while ready with an empty log", () => {
    const onSuggestion = vi.fn();
    render(
      <ConversationLog log={[]} phase="ready" onSuggestion={onSuggestion} />,
    );
    const hero = screen.getByTestId("conversation-empty");
    expect(within(hero).getByText("무엇을 만들까요?")).toBeInTheDocument();
    const chip = within(hero).getAllByRole("button")[0];
    fireEvent.click(chip);
    expect(onSuggestion).toHaveBeenCalledWith(chip.textContent);
  });

  it("disables the chips while sending is gated off", () => {
    render(
      <ConversationLog
        log={[]}
        phase="ready"
        onSuggestion={() => {}}
        suggestionsEnabled={false}
      />,
    );
    for (const chip of screen.getAllByRole("button")) {
      expect(chip).toBeDisabled();
    }
  });

  it("hides the hero once the log has entries, while busy, or during warmup", () => {
    const store = createPanelStore();
    store.log("you", "안녕?");
    const withLog = render(
      <ConversationLog log={store.getState().log} phase="ready" />,
    );
    expect(withLog.queryByTestId("conversation-empty")).not.toBeInTheDocument();
    withLog.unmount();

    const thinking = render(<ConversationLog log={[]} phase="thinking" />);
    expect(thinking.queryByTestId("conversation-empty")).not.toBeInTheDocument();
    thinking.unmount();

    render(<ConversationLog log={[]} phase="ready" ragLoading />);
    expect(screen.queryByTestId("conversation-empty")).not.toBeInTheDocument();
  });
});

describe("ConversationLog — message virtualization", () => {
  it("keeps only the viewport window mounted for a long conversation", () => {
    const store = createPanelStore();
    for (let index = 0; index < 200; index += 1) {
      store.log(index % 2 === 0 ? "you" : "agent", `메시지 ${index}`);
    }

    const { container } = render(
      <ConversationLog log={store.getState().log} phase="ready" />,
    );

    expect(store.getState().log).toHaveLength(200);
    const mounted = container.querySelectorAll("[data-virtual-row]");
    expect(mounted.length).toBeGreaterThan(0);
    expect(mounted.length).toBeLessThan(50);
  });
});

// ---- RAG warmup shimmer: while the RAG model loads (~19s after server boot)
// sending is blocked (store gate), and the conversation area shows a Shimmer
// "RAG 모델 준비 중…" row so the locked input is explained.
describe("ConversationLog — RAG warmup shimmer", () => {
  it("shows the RAG waiting shimmer while the model loads", () => {
    render(<ConversationLog log={[]} phase="ready" ragLoading />);
    const waiting = screen.getByTestId("rag-waiting");
    expect(waiting).toBeInTheDocument();
    expect(waiting).toHaveTextContent("RAG 모델 준비 중…");
  });

  it("does not show it when loading is over (prop absent)", () => {
    render(<ConversationLog log={[]} phase="ready" />);
    expect(screen.queryByTestId("rag-waiting")).not.toBeInTheDocument();
  });
});
