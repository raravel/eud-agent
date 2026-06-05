/**
 * Agent activity stream (features/06 ## Behaviors → Agent stream): a live
 * activity line rendered under the latest user message from the turn's
 * `agent_event`s — "도구 호출 n건 · 현재: <last tool/detail>". When the turn ends
 * (the store phase leaves `thinking`) it collapses into a summary row.
 *
 * Contract (Step B implements `@/components/AgentStream`):
 *   export interface AgentActivity { kind: string; detail: string; }
 *   export interface AgentStreamProps {
 *     events: AgentActivity[];   // the current turn's agent_event stream
 *     live: boolean;             // true while phase === "thinking"
 *   }
 *   export function AgentStream(props): JSX.Element | null;
 *
 * "도구 호출" counts tool-call events (kind "tool_call"); the "현재" detail is the
 * latest event's detail (or kind when detail is empty). With zero events the
 * component renders nothing (null).
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { AgentStream } from "@/components/AgentStream";

describe("AgentStream — live line", () => {
  it("renders nothing when there are no events", () => {
    const { container } = render(<AgentStream events={[]} live={true} />);
    expect(container.firstChild).toBeNull();
  });

  it("shows the tool-call count and the current (latest) detail while live", () => {
    render(
      <AgentStream
        live={true}
        events={[
          { kind: "tool_call", detail: "dat_set unit hp" },
          { kind: "tool_call", detail: "file_write main.eps" },
        ]}
      />,
    );
    expect(screen.getByText(/도구 호출 2건/)).toBeInTheDocument();
    expect(screen.getByText(/file_write main\.eps/)).toBeInTheDocument();
  });

  it("counts only tool_call kinds toward 도구 호출 n건", () => {
    render(
      <AgentStream
        live={true}
        events={[
          { kind: "thinking", detail: "" },
          { kind: "tool_call", detail: "dat_set unit hp" },
          { kind: "tool_result", detail: "ok" },
        ]}
      />,
    );
    expect(screen.getByText(/도구 호출 1건/)).toBeInTheDocument();
  });

  it("shows a spinner while live", () => {
    render(
      <AgentStream
        live={true}
        events={[{ kind: "tool_call", detail: "dat_set unit hp" }]}
      />,
    );
    expect(screen.getByRole("status")).toBeInTheDocument();
  });
});

describe("AgentStream — collapsed summary", () => {
  it("collapses into a summary row (no spinner) once the turn ends", () => {
    render(
      <AgentStream
        live={false}
        events={[
          { kind: "tool_call", detail: "dat_set unit hp" },
          { kind: "tool_call", detail: "file_write main.eps" },
        ]}
      />,
    );
    // Summary still reports the count, but no live spinner.
    expect(screen.getByText(/도구 호출 2건/)).toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
