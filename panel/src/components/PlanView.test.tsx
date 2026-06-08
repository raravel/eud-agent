/**
 * Plan review card (features/06 ## UI layout + Behaviors → Plan review, EUD-074):
 *   a markdown plan card (Streamdown) + a [승인] button (`plan_approve{}`).
 *   The feedback textarea and the [수정요청] button are REMOVED (user decision
 *   2026-06-05): plan feedback flows through the MAIN prompt input — typing in
 *   the prompt during plan_review sends `plan_feedback{text}` (App routes it).
 *
 * Contract (`@/components/PlanView`):
 *   export interface PlanViewProps {
 *     plan: PlanState;     // { markdown, revision }
 *     pending: boolean;    // a turn is in flight (disable 승인)
 *     onApprove(): void;   // App fires WS plan_approve{}
 *   }
 *
 * Revision replacement is owned by the STORE (a higher revision replaces the
 * active card), so the component is a thin renderer of whatever `plan` it gets.
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
    render(<PlanView plan={rev1} pending={false} onApprove={() => {}} />);
    expect(screen.getByText("계획 1")).toBeInTheDocument();
    expect(screen.getByText("첫 번째 단계")).toBeInTheDocument();
    expect(screen.getByText("두 번째 단계")).toBeInTheDocument();
  });

  it("renders a fenced code block as styled text (not interpreted)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "본문\n\n```eps\nfunction tp() {}\n```",
    };
    render(<PlanView plan={plan} pending={false} onApprove={() => {}} />);
    expect(screen.getByText(/function tp/)).toBeInTheDocument();
  });

  it("never injects a live <script> node (Streamdown sanitizes untrusted markdown)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "안전 <script>alert(1)</script> 텍스트",
    };
    const { container } = render(
      <PlanView plan={plan} pending={false} onApprove={() => {}} />,
    );
    expect(container.querySelector("script")).toBeNull();
    expect(screen.getByText(/텍스트/)).toBeInTheDocument();
  });

  it("renders via the AI-Elements Plan component (data-slot=plan)", () => {
    const { container } = render(
      <PlanView plan={rev1} pending={false} onApprove={() => {}} />,
    );
    expect(container.querySelector('[data-slot="plan"]')).not.toBeNull();
  });

  it("renders evidence citation links as real anchors with href (EUD-090)", () => {
    // Streamdown's default linkSafety renders links as href-LESS buttons +
    // confirm modal — the live session showed citations as dead text. The
    // Response wrapper disables it: links must be <a href target="_blank">
    // (the WebView2 host routes the new-window request to the default browser).
    const plan: PlanState = {
      revision: 1,
      markdown:
        "- 이유: 대기열 인식 패턴이 검증되어 있습니다. " +
        "(근거: [EPS로 배쉬 스킬을 만들어보자.](https://cafe.naver.com/f-e/cafes/17046257/articles/137536))",
    };
    const { container } = render(
      <PlanView plan={plan} pending={false} onApprove={() => {}} />,
    );
    const anchor = container.querySelector('a[data-streamdown="link"]');
    expect(anchor).not.toBeNull();
    expect(anchor?.getAttribute("href")).toBe(
      "https://cafe.naver.com/f-e/cafes/17046257/articles/137536",
    );
    expect(anchor?.getAttribute("target")).toBe("_blank");
    expect(anchor?.textContent).toBe("EPS로 배쉬 스킬을 만들어보자.");
    // The href-less link-safety BUTTON shape must be gone.
    expect(
      container.querySelector('button[data-streamdown="link"]'),
    ).toBeNull();
  });
});

describe("PlanView — revision replacement (store-driven)", () => {
  it("rev2 replaces rev1 content when the plan prop changes", () => {
    const { rerender } = render(
      <PlanView plan={rev1} pending={false} onApprove={() => {}} />,
    );
    expect(screen.getByText("계획 1")).toBeInTheDocument();

    rerender(<PlanView plan={rev2} pending={false} onApprove={() => {}} />);
    expect(screen.getByText("계획 2")).toBeInTheDocument();
    expect(screen.getByText("수정된 내용입니다.")).toBeInTheDocument();
    expect(screen.queryByText("계획 1")).not.toBeInTheDocument();
    expect(screen.queryByText("첫 번째 단계")).not.toBeInTheDocument();
  });
});

describe("PlanView — no embedded feedback input (EUD-074)", () => {
  it("renders NO feedback textarea and NO 수정요청 button", () => {
    render(<PlanView plan={rev1} pending={false} onApprove={() => {}} />);
    // Feedback flows through the MAIN prompt input now.
    expect(screen.queryByLabelText("피드백 입력")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "수정요청" }),
    ).not.toBeInTheDocument();
  });
});

describe("PlanView — approve dispatch", () => {
  it("[승인] calls onApprove with no payload", async () => {
    const onApprove = vi.fn();
    render(<PlanView plan={rev1} pending={false} onApprove={onApprove} />);
    await userEvent.click(screen.getByRole("button", { name: "승인" }));
    expect(onApprove).toHaveBeenCalledWith();
  });
});

describe("PlanView — pending state", () => {
  it("disables 승인 while a turn is in flight", () => {
    render(<PlanView plan={rev1} pending={true} onApprove={() => {}} />);
    expect(screen.getByRole("button", { name: "승인" })).toBeDisabled();
  });
});
