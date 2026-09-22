/**
 * Plan tab body (features/staged-workflow-plan.md ## Phase 2 Panel, EUD-074):
 *   a full-height markdown plan (Streamdown) + a [승인] button (`plan_approve{}`)
 *   in the tab header while the plan awaits review. The feedback textarea and
 *   the [수정요청] button are REMOVED (user decision 2026-06-05): plan feedback
 *   flows through the MAIN prompt input — typing in the prompt during
 *   plan_review sends `plan_feedback{text}` (App routes it).
 *
 * Contract (`@/components/PlanView`):
 *   export interface PlanViewProps {
 *     plan: PlanState;                // { markdown, revision }
 *     artifact?: WorkflowPlanArtifact;// staged-workflow plan details
 *     reviewable: boolean;            // phase === plan_review → 승인 shown
 *     pending: boolean;               // approve command in flight (disable 승인)
 *     onApprove(): void;              // App invokes plan_approve{}
 *   }
 *
 * Revision replacement and tab lifecycle are App-owned, so the component is a
 * controlled renderer of the selected session's active plan.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PlanView, type PlanViewProps } from "@/components/PlanView";
import type { PlanState } from "@/state/store";

const rev1: PlanState = {
  revision: 1,
  markdown: "# 계획 1\n\n- 첫 번째 단계\n- 두 번째 단계",
};

const rev2: PlanState = {
  revision: 2,
  markdown: "# 계획 2\n\n수정된 내용입니다.",
};

const defaultPlanViewProps: Omit<PlanViewProps, "plan"> = {
  reviewable: true,
  pending: false,
  onApprove: () => {},
};

describe("PlanView — markdown render", () => {
  it("renders the plan markdown content (heading + list items)", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} />);
    expect(screen.getByText("계획 1")).toBeInTheDocument();
    expect(screen.getByText("첫 번째 단계")).toBeInTheDocument();
    expect(screen.getByText("두 번째 단계")).toBeInTheDocument();
  });

  it("renders a fenced code block as styled text (not interpreted)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "본문\n\n```eps\nfunction tp() {}\n```",
    };
    render(<PlanView {...defaultPlanViewProps} plan={plan} />);
    expect(screen.getByText(/function tp/)).toBeInTheDocument();
  });

  it("renders Mermaid in the plan review surface", async () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "```mermaid\nflowchart LR\nA[요청] --> B[실행]\n```",
    };
    const { container } = render(
      <PlanView {...defaultPlanViewProps} plan={plan} />,
    );

    await waitFor(() => {
      expect(container.querySelector('[data-streamdown="mermaid"]')).not.toBeNull();
    });
  });

  it("never injects a live <script> node (Streamdown sanitizes untrusted markdown)", () => {
    const plan: PlanState = {
      revision: 1,
      markdown: "안전 <script>alert(1)</script> 텍스트",
    };
    const { container } = render(
      <PlanView {...defaultPlanViewProps} plan={plan} />,
    );
    expect(container.querySelector("script")).toBeNull();
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
      <PlanView {...defaultPlanViewProps} plan={plan} />,
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
      <PlanView {...defaultPlanViewProps} plan={rev1} />,
    );
    expect(screen.getByText("계획 1")).toBeInTheDocument();
    expect(screen.getByText("계획안 (rev 1)")).toBeInTheDocument();

    rerender(<PlanView {...defaultPlanViewProps} plan={rev2} />);
    expect(screen.getByText("계획 2")).toBeInTheDocument();
    expect(screen.getByText("계획안 (rev 2)")).toBeInTheDocument();
    expect(screen.getByText("수정된 내용입니다.")).toBeInTheDocument();
    expect(screen.queryByText("계획 1")).not.toBeInTheDocument();
    expect(screen.queryByText("첫 번째 단계")).not.toBeInTheDocument();
  });
});

describe("PlanView — no embedded feedback input (EUD-074)", () => {
  it("renders NO feedback textarea and NO 수정요청 button", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} />);
    // Feedback flows through the MAIN prompt input now.
    expect(screen.queryByLabelText("피드백 입력")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "수정요청" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("수정하려면 아래 입력창에 피드백을 입력하세요."),
    ).toBeInTheDocument();
  });
});

describe("PlanView — approve dispatch", () => {
  it("[승인] calls onApprove", async () => {
    const onApprove = vi.fn();
    render(
      <PlanView {...defaultPlanViewProps} plan={rev1} onApprove={onApprove} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "승인" }));
    expect(onApprove).toHaveBeenCalledTimes(1);
  });

  it("keeps approval in the tab header, outside the scrollable plan body", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} />);
    const section = screen.getByRole("region", { name: "계획 검토" });
    const actions = screen.getByTestId("plan-actions");
    const body = screen.getByText("첫 번째 단계").closest(".overflow-y-auto");

    expect(section).toContainElement(actions);
    expect(body).not.toBeNull();
    expect(body).not.toContainElement(actions);
    expect(actions).toHaveClass("shrink-0");
  });
});

describe("PlanView — pending state", () => {
  it("disables 승인 while the approve command is in flight", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} pending={true} />);
    expect(screen.getByRole("button", { name: "승인" })).toBeDisabled();
  });
});

describe("PlanView — read-only after review", () => {
  it("hides 승인 and shows the read-only badge once the plan left review", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} reviewable={false} />);
    expect(screen.queryByRole("button", { name: "승인" })).not.toBeInTheDocument();
    expect(screen.queryByTestId("plan-actions")).not.toBeInTheDocument();
    expect(screen.getByText("읽기 전용")).toBeInTheDocument();
    expect(screen.getByText("계획 1")).toBeInTheDocument();
  });

  it("labels an approved artifact as 승인됨", () => {
    render(
      <PlanView
        {...defaultPlanViewProps}
        plan={rev1}
        reviewable={false}
        artifact={{
          path: "plans/req-1.md",
          revision: 1,
          sha256: "d".repeat(64),
          approvedSha256: "d".repeat(64),
          title: "",
          acceptanceCriteria: [],
          deep: false,
          iterations: 1,
        }}
      />,
    );
    expect(screen.getByText("승인됨 · 읽기 전용")).toBeInTheDocument();
  });
});

describe("PlanView — staged-workflow plan artifact", () => {
  it("renders acceptance criteria, critic verdict/summary, and the deep-planning badge", () => {
    render(
      <PlanView
        {...defaultPlanViewProps}
        plan={rev2}
        artifact={{
          path: "plans/req-1.md",
          revision: 2,
          sha256: "d".repeat(64),
          title: "미네랄 지급 트리거 추가",
          acceptanceCriteria: ["게임 시작 시 미네랄 1000 지급", "빌드 통과"],
          criticVerdict: "revise",
          criticSummary: "검증 단계가 없는 항목이 있습니다.",
          deep: true,
          iterations: 2,
        }}
      />,
    );
    expect(screen.getByText("계획안 (rev 2)")).toBeInTheDocument();
    expect(screen.getByText("미네랄 지급 트리거 추가")).toBeInTheDocument();
    const criteria = screen.getByRole("list", { name: "수용 기준" });
    expect(within(criteria).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "게임 시작 시 미네랄 1000 지급",
      "빌드 통과",
    ]);
    expect(screen.getByText("비평 수정 요청")).toBeInTheDocument();
    expect(screen.getByText("검증 단계가 없는 항목이 있습니다.")).toBeInTheDocument();
    expect(screen.getByText("심층 계획 · 2회")).toBeInTheDocument();
  });

  it("keeps the event-only path unchanged without an artifact", () => {
    render(<PlanView {...defaultPlanViewProps} plan={rev1} />);
    expect(screen.queryByRole("list", { name: "수용 기준" })).toBeNull();
    expect(screen.queryByText("심층 계획")).toBeNull();
  });
});
