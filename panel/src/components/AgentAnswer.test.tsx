/**
 * Live agent answer bubble (features/06 ## Behaviors → Answer prominence):
 * the streamed `delta` answer text renders into a PROMINENT (foreground)
 * AI-Elements Message/Response bubble via Streamdown, re-rendering as the text
 * grows. Agent answers are the most visible text in the log (foreground), in
 * contrast to muted system/progress rows.
 *
 * Contract (Step B implements `@/components/AgentAnswer`):
 *   export interface AgentAnswerProps { text: string; }
 *   export function AgentAnswer(props): JSX.Element | null;  // null when text === ""
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { AgentAnswer } from "@/components/AgentAnswer";

describe("AgentAnswer — prominent streamed answer", () => {
  it("renders nothing for empty text", () => {
    const { container } = render(<AgentAnswer text="" />);
    expect(container.firstChild).toBeNull();
  });

  it("renders the answer markdown text", () => {
    render(<AgentAnswer text="**HP**를 80으로 변경했습니다." />);
    expect(screen.getByText(/80으로 변경했습니다/)).toBeInTheDocument();
  });

  it("renders the answer as prominent foreground text, not muted", () => {
    const { container } = render(<AgentAnswer text="중요한 답변" />);
    // Prominence is the styling inversion (features/06): the answer carries the
    // foreground class and NOT the muted-foreground class that system/progress
    // rows use.
    const root = container.firstElementChild as HTMLElement;
    const cls = root.className;
    expect(cls).toContain("text-foreground");
    expect(cls).not.toContain("text-muted-foreground");
  });

  it("re-renders as the streamed text grows (Streamdown live)", () => {
    const { rerender } = render(<AgentAnswer text="안녕" />);
    expect(screen.getByText("안녕")).toBeInTheDocument();
    rerender(<AgentAnswer text="안녕하세요 반갑습니다" />);
    expect(screen.getByText(/반갑습니다/)).toBeInTheDocument();
  });
});
