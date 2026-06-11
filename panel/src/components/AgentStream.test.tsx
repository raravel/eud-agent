/**
 * Agent activity stream (features/06 ## Behaviors → Agent stream), rebuilt on
 * the vendored AI Elements (Reasoning + Tool) + Streamdown (decision 06):
 *   - `reasoning` text renders into the AI-Elements Reasoning block: dim,
 *     collapsible, auto-open while reasoning streams, collapses once the answer
 *     starts;
 *   - `tool_call`/`tool_result` render as Tool rows by tool name, with a
 *     "도구 호출 n건" summary;
 *   - raw internal kind identifiers (delta/answer/token_usage/turn_done/…) MUST
 *     NEVER appear as literal UI text.
 *
 * Contract (Step B implements `@/components/AgentStream`):
 *   export interface AgentTool { id: string; name: string; state: "running" | "done"; detail?: string; }
 *   export interface AgentStreamProps {
 *     reasoning: string;       // accumulated reasoning delta text
 *     answerStarted: boolean;  // true once answer delta text has begun
 *     tools: AgentTool[];      // tool_call/tool_result rows
 *     live: boolean;           // true while the turn is in flight
 *   }
 *   export function AgentStream(props): JSX.Element | null;
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AgentStream } from "@/components/AgentStream";

describe("AgentStream — reasoning surface", () => {
  it("renders nothing with no reasoning and no tools", () => {
    const { container } = render(
      <AgentStream reasoning="" answerStarted={false} tools={[]} live={true} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders accumulated reasoning text (dim/collapsible Reasoning block)", () => {
    render(
      <AgentStream
        reasoning="유닛 속성을 먼저 확인합니다."
        answerStarted={false}
        tools={[]}
        live={true}
      />,
    );
    expect(
      screen.getByText(/유닛 속성을 먼저 확인합니다\./),
    ).toBeInTheDocument();
  });

  it("auto-opens the reasoning block while streaming (content visible)", () => {
    render(
      <AgentStream
        reasoning="추론 중 텍스트"
        answerStarted={false}
        tools={[]}
        live={true}
      />,
    );
    expect(screen.getByText(/추론 중 텍스트/)).toBeInTheDocument();
  });

  it("collapses the reasoning block once the answer starts", () => {
    render(
      <AgentStream
        reasoning="감춰질 추론"
        answerStarted={true}
        tools={[]}
        live={true}
      />,
    );
    // Radix Collapsible removes content from the tree when closed (no
    // forceMount), so the collapsed reasoning text is absent from the DOM.
    expect(screen.queryByText("감춰질 추론")).not.toBeInTheDocument();
  });

  it("keeps completed reasoning CLOSED by default (live=false, post-archive answerStarted reset)", () => {
    // The store's turn-end archive resets answerStarted to false while keeping the
    // reasoning text; a completed (not-live) reasoning block must NOT re-open.
    render(
      <AgentStream
        reasoning="완료된 추론"
        answerStarted={false}
        tools={[]}
        live={false}
      />,
    );
    expect(screen.queryByText("완료된 추론")).not.toBeInTheDocument();
  });

  it("lets the user open completed reasoning on demand (live=false)", async () => {
    const user = userEvent.setup();
    render(
      <AgentStream
        reasoning="펼칠 완료 추론"
        answerStarted={false}
        tools={[]}
        live={false}
      />,
    );
    expect(screen.queryByText("펼칠 완료 추론")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button"));
    expect(screen.getByText(/펼칠 완료 추론/)).toBeInTheDocument();
  });

  it("lets the user re-expand the collapsed reasoning after the answer starts (F1)", async () => {
    const user = userEvent.setup();
    render(
      <AgentStream
        reasoning="다시 펼칠 추론"
        answerStarted={true}
        tools={[]}
        live={true}
      />,
    );
    // Auto-collapsed because the answer started — content is hidden.
    expect(screen.queryByText("다시 펼칠 추론")).not.toBeInTheDocument();
    // The collapsible trigger stays clickable; clicking it re-expands the block.
    await user.click(screen.getByRole("button"));
    expect(screen.getByText(/다시 펼칠 추론/)).toBeInTheDocument();
  });
});

describe("AgentStream — tool rows", () => {
  it("renders a Tool row per tool with the tool name", () => {
    render(
      <AgentStream
        reasoning=""
        answerStarted={false}
        live={true}
        tools={[
          { id: "t1", name: "dat_set unit hp", state: "done" },
          { id: "t2", name: "file_write main.eps", state: "running" },
        ]}
      />,
    );
    expect(screen.getByText(/dat_set unit hp/)).toBeInTheDocument();
    expect(screen.getByText(/file_write main\.eps/)).toBeInTheDocument();
  });

  it("keeps a 도구 호출 n건 summary", () => {
    render(
      <AgentStream
        reasoning=""
        answerStarted={false}
        live={true}
        tools={[
          { id: "t1", name: "dat_set", state: "done" },
          { id: "t2", name: "file_write", state: "done" },
        ]}
      />,
    );
    expect(screen.getByText(/도구 호출 2건/)).toBeInTheDocument();
  });
});

describe("AgentStream — tool args/result (EUD-068)", () => {
  it("shows the call args and result text when the row is expanded", async () => {
    const user = userEvent.setup();
    render(
      <AgentStream
        reasoning=""
        answerStarted={false}
        live={true}
        tools={[
          {
            id: "t1",
            name: "dat_set",
            state: "done",
            args: '{"dat":"units","param":"Hit Points","value":20480}',
            detail: "OK: units|Hit Points|0 = 20480",
          },
        ]}
      />,
    );
    // Collapsed by default — expand the row.
    await user.click(screen.getByRole("button", { name: /dat_set/ }));
    expect(screen.getByText("요청")).toBeInTheDocument();
    expect(
      screen.getByText('{"dat":"units","param":"Hit Points","value":20480}'),
    ).toBeInTheDocument();
    expect(screen.getByText("결과")).toBeInTheDocument();
    expect(
      screen.getByText("OK: units|Hit Points|0 = 20480"),
    ).toBeInTheDocument();
  });

  it("renders a 실패 badge for a failed tool row", () => {
    render(
      <AgentStream
        reasoning=""
        answerStarted={false}
        live={true}
        tools={[
          {
            id: "t1",
            name: "dat_set",
            state: "failed",
            detail: "ERROR: invalid dat name",
          },
        ]}
      />,
    );
    expect(screen.getByText("실패")).toBeInTheDocument();
  });
});

describe("AgentStream — no raw kind leak", () => {
  it("never surfaces raw internal kind identifiers as literal text", () => {
    const { container } = render(
      <AgentStream
        reasoning="정상 추론 텍스트"
        answerStarted={false}
        live={true}
        tools={[{ id: "t1", name: "dat_set", state: "running" }]}
      />,
    );
    const text = container.textContent ?? "";
    for (const raw of [
      "delta",
      "token_usage",
      "turn_done",
      "item_started",
      "item_completed",
    ]) {
      expect(text).not.toContain(raw);
    }
  });
});
