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
