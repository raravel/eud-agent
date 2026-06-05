/**
 * Plan review card (features/06 ## UI layout + Behaviors → Plan review):
 *   a markdown plan card; a 피드백 textarea that sends `plan_feedback{text}`
 *   (the panel stays in plan_review until the next `plan{revision+1}` replaces
 *   the card — the store already handles revision replacement, EUD-058); a
 *   [수정요청] button (feedback) + a [승인] button (`plan_approve{}`). Korean
 *   labels. Markdown is rendered via a MINIMAL safe line-based renderer (no
 *   dangerouslySetInnerHTML, no new runtime dep).
 *
 * Contract (Step B implements `@/components/PlanView`):
 *   export interface PlanViewProps {
 *     plan: PlanState;                 // { markdown, revision }
 *     pending: boolean;                // a turn is in flight (disable controls)
 *     onFeedback(text: string): void;  // App fires WS plan_feedback{text}
 *     onApprove(): void;               // App fires WS plan_approve{}
 *   }
 *   export function PlanView(props): JSX.Element;
 *
 * Revision replacement is owned by the STORE (a higher revision replaces the
 * active card), so the component is a thin renderer of whatever `plan` it gets;
 * re-rendering with a rev2 plan must show rev2 content and drop rev1 content.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PlanView } from "@/components/PlanView";
import type { PlanState } from "@/state/store";

const rev1: PlanState = {
  revision: 1,
  markdown: "# 계획 1\n\n- 첫 번째 단계\n- 두 번째 단계",
};

const rev2: PlanState = {
  revision: 2,
  markdown: "# 계획 2\n\n수정된 내용입니다.",
};

describe("PlanView — markdown render", () => {
  it("renders the plan markdown content (heading + list items)", () => {
    render(
      <PlanView plan={rev1} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(screen.getByText("계획 1")).toBeInTheDocument();
    expect(screen.getByText("첫 번째 단계")).toBeInTheDocument();
    expect(screen.getByText("두 번째 단계")).toBeInTheDocument();
  });

  it("renders a fenced code block as styled text (not interpreted)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "본문\n\n```eps\nfunction tp() {}\n```",
    };
    render(
      <PlanView plan={plan} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(screen.getByText(/function tp/)).toBeInTheDocument();
  });

  it("never injects a live <script> node (Streamdown sanitizes untrusted markdown)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "안전 <script>alert(1)</script> 텍스트",
    };
    const { container } = render(
      <PlanView plan={plan} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    // Streamdown (rehype-sanitize) renders untrusted markdown safely — no live
    // <script> node is ever created.
    expect(container.querySelector("script")).toBeNull();
    // The surrounding safe text still renders.
    expect(screen.getByText(/텍스트/)).toBeInTheDocument();
  });

  it("renders via the AI-Elements Plan component (data-slot=plan)", () => {
    const { container } = render(
      <PlanView plan={rev1} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(container.querySelector('[data-slot="plan"]')).not.toBeNull();
  });
});

describe("PlanView — revision replacement (store-driven)", () => {
  it("rev2 replaces rev1 content when the plan prop changes", () => {
    const { rerender } = render(
      <PlanView plan={rev1} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(screen.getByText("계획 1")).toBeInTheDocument();

    rerender(
      <PlanView plan={rev2} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(screen.getByText("계획 2")).toBeInTheDocument();
    expect(screen.getByText("수정된 내용입니다.")).toBeInTheDocument();
    // rev1 content is gone (the card was replaced, not appended).
    expect(screen.queryByText("계획 1")).not.toBeInTheDocument();
    expect(screen.queryByText("첫 번째 단계")).not.toBeInTheDocument();
  });
});

describe("PlanView — feedback dispatch", () => {
  it("[수정요청] sends the textarea text via onFeedback", async () => {
    const onFeedback = vi.fn();
    render(
      <PlanView plan={rev1} pending={false} onFeedback={onFeedback} onApprove={() => {}} />,
    );
    await userEvent.type(screen.getByLabelText("피드백 입력"), "HP를 100으로");
    await userEvent.click(screen.getByRole("button", { name: "수정요청" }));
    expect(onFeedback).toHaveBeenCalledWith("HP를 100으로");
  });

  it("does not dispatch feedback for empty/whitespace text", async () => {
    const onFeedback = vi.fn();
    render(
      <PlanView plan={rev1} pending={false} onFeedback={onFeedback} onApprove={() => {}} />,
    );
    await userEvent.type(screen.getByLabelText("피드백 입력"), "   ");
    await userEvent.click(screen.getByRole("button", { name: "수정요청" }));
    expect(onFeedback).not.toHaveBeenCalled();
  });

  it("clears the textarea after a feedback dispatch", async () => {
    render(
      <PlanView plan={rev1} pending={false} onFeedback={() => {}} onApprove={() => {}} />,
    );
    const ta = screen.getByLabelText("피드백 입력") as HTMLTextAreaElement;
    await userEvent.type(ta, "다시");
    await userEvent.click(screen.getByRole("button", { name: "수정요청" }));
    expect(ta.value).toBe("");
  });
});

describe("PlanView — approve dispatch", () => {
  it("[승인] calls onApprove with no payload", async () => {
    const onApprove = vi.fn();
    render(
      <PlanView plan={rev1} pending={false} onFeedback={() => {}} onApprove={onApprove} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "승인" }));
    expect(onApprove).toHaveBeenCalledWith();
  });
});

describe("PlanView — pending state", () => {
  it("disables both buttons while a turn is in flight", () => {
    render(
      <PlanView plan={rev1} pending={true} onFeedback={() => {}} onApprove={() => {}} />,
    );
    expect(screen.getByRole("button", { name: "수정요청" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "승인" })).toBeDisabled();
  });
});
