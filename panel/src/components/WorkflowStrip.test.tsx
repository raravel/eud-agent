/**
 * Stage strip contracts (features/staged-workflow-plan.md ## Phase 2 Panel):
 * the active step carries `aria-current="step"`, earlier steps read as
 * completed, attempt counters and the deep-planning badge render from the
 * snapshot, cancel shows only while a stage job is busy, and the interrupted
 * controls call the resume/restart handlers (disabled while a command is
 * pending).
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { WorkflowStrip, type WorkflowStripProps } from "@/components/WorkflowStrip";
import type { WorkflowEvent } from "@/lib/ipc";

function workflow(overrides: Partial<WorkflowEvent> = {}): WorkflowEvent {
  return {
    requestId: "req-1",
    stage: "research",
    route: "pipeline",
    acceptanceCriteria: [],
    critiqueRounds: 0,
    verifyAttempts: 0,
    deepPlanning: false,
    ...overrides,
  };
}

function renderStrip(overrides: Partial<WorkflowStripProps> = {}) {
  const props: WorkflowStripProps = {
    workflow: workflow(),
    phase: "research",
    actionBusy: false,
    onCancel: vi.fn(),
    onResume: vi.fn(),
    onRestart: vi.fn(),
    ...overrides,
  };
  return { ...render(<WorkflowStrip {...props} />), props };
}

function steps() {
  const nav = screen.getByRole("navigation", { name: "작업 단계" });
  return within(nav).getAllByRole("listitem");
}

describe("WorkflowStrip", () => {
  it("renders the seven steps in order with aria-current on the active one", () => {
    renderStrip();
    const items = steps();
    expect(items.map((item) => item.textContent)).toEqual([
      "파악",
      "조사",
      "계획",
      "승인",
      "실행",
      "검증",
      "검토",
    ]);
    expect(items[0]).toHaveAttribute("data-state", "completed");
    expect(items[1]).toHaveAttribute("aria-current", "step");
    expect(items[1]).toHaveAttribute("data-state", "active");
    expect(items[2]).toHaveAttribute("data-state", "pending");
    expect(items.filter((item) => item.getAttribute("aria-current") === "step")).toHaveLength(1);
  });

  it("maps clarify and critique onto the 파악 / 계획 steps", () => {
    const { unmount } = renderStrip({
      workflow: workflow({ stage: "clarify" }),
      phase: "thinking",
    });
    expect(steps()[0]).toHaveAttribute("aria-current", "step");
    unmount();
    renderStrip({
      workflow: workflow({ stage: "critique", critiqueRounds: 1 }),
      phase: "planning",
    });
    const planning = steps()[2];
    expect(planning).toHaveAttribute("aria-current", "step");
    expect(planning).toHaveTextContent("비평 1회");
  });

  it("shows the verify attempt counter and the deep-planning badge", () => {
    renderStrip({
      workflow: workflow({
        stage: "verifying",
        verifyAttempts: 2,
        deepPlanning: true,
      }),
      phase: "verifying",
    });
    expect(steps()[5]).toHaveTextContent("검증2/2");
    expect(screen.getByText("심층 계획")).toBeInTheDocument();
  });

  it("offers cancel only while a stage job is busy", async () => {
    const { props, unmount } = renderStrip();
    await userEvent.click(screen.getByRole("button", { name: "작업 중단" }));
    expect(props.onCancel).toHaveBeenCalledOnce();
    unmount();
    renderStrip({
      workflow: workflow({ stage: "plan_review" }),
      phase: "plan_review",
    });
    expect(screen.queryByRole("button", { name: "작업 중단" })).toBeNull();
    expect(steps()[3]).toHaveAttribute("aria-current", "step");
  });

  it("interrupted: highlights the cut stage and routes resume/restart", async () => {
    const { props } = renderStrip({
      workflow: workflow({ stage: "interrupted", interruptedStage: "planning" }),
      phase: "interrupted",
    });
    const planning = steps()[2];
    expect(planning).toHaveAttribute("aria-current", "step");
    expect(planning).toHaveAttribute("data-state", "interrupted");
    expect(screen.getByRole("status")).toHaveTextContent("작업이 중단되었습니다.");
    expect(screen.queryByRole("button", { name: "작업 중단" })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "이어서 진행" }));
    expect(props.onResume).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "처음부터" }));
    expect(props.onRestart).toHaveBeenCalledOnce();
  });

  it("disables the interrupted controls while a command is pending", () => {
    renderStrip({
      workflow: workflow({ stage: "interrupted", interruptedStage: "research" }),
      phase: "interrupted",
      actionBusy: true,
    });
    expect(screen.getByRole("button", { name: "이어서 진행" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "처음부터" })).toBeDisabled();
  });
});

describe("WorkflowStrip cancelled", () => {
  it("shows 취소됨 as a terminal state with only the restart control", async () => {
    const { props } = renderStrip({
      workflow: workflow({ stage: "cancelled", interruptedStage: "planning" }),
      phase: "ready",
    });
    expect(screen.getByRole("status")).toHaveTextContent("취소됨");
    expect(steps()[2]).toHaveAttribute("data-state", "cancelled");
    expect(screen.queryByRole("button", { name: "이어서 진행" })).toBeNull();
    expect(screen.queryByRole("button", { name: "작업 중단" })).toBeNull();
    const restart = screen.getByRole("button", { name: "처음부터" });
    expect(restart).toBeEnabled();
    await userEvent.click(restart);
    expect(props.onRestart).toHaveBeenCalledOnce();
  });
});

describe("WorkflowStrip after resume was sent", () => {
  it("drops the interrupted controls and spins the resumed step once the phase is busy", () => {
    render(
      <WorkflowStrip
        workflow={workflow({ stage: "interrupted", interruptedStage: "research" })}
        phase="research"
        actionBusy={false}
        onCancel={vi.fn()}
        onResume={vi.fn()}
        onRestart={vi.fn()}
      />,
    );
    expect(screen.queryByRole("button", { name: "이어서 진행" })).toBeNull();
    expect(screen.getByRole("button", { name: "작업 중단" })).toBeInTheDocument();
    const active = steps()[1];
    expect(active).toHaveAttribute("aria-current", "step");
    expect(active).toHaveAttribute("data-state", "active");
  });
});
