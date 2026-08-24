import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { HarnessStatusCard } from "@/components/HarnessStatusCard";
import type { HarnessJobView } from "@/lib/protocol";

function job(overrides: Partial<HarnessJobView>): HarnessJobView {
  return {
    id: "harness-1",
    sessionId: "session-1",
    sourceRequestId: "req-code",
    status: "waiting_runtime",
    runtimeVerification: "waiting",
    attempts: 0,
    createdAt: 1,
    updatedAt: 1,
    memoryFiles: [],
    dismissed: false,
    ...overrides,
  };
}

describe("HarnessStatusCard", () => {
  it("keeps runtime-sensitive changes pending until the user confirms in-game verification", async () => {
    const onRuntimeConfirm = vi.fn();
    const onSkip = vi.fn();
    render(
      <HarnessStatusCard
        jobs={[job({})]}
        pendingJobId={null}
        onRuntimeConfirm={onRuntimeConfirm}
        onSkip={onSkip}
        onRetry={vi.fn()}
        onDismiss={vi.fn()}
        onDecide={vi.fn()}
      />,
    );

    expect(screen.getByText("인게임 검증 대기")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "건너뛰기" }));
    expect(onSkip).toHaveBeenCalledWith("harness-1");
    await userEvent.click(screen.getByRole("button", { name: "인게임 검증 완료" }));
    expect(onRuntimeConfirm).toHaveBeenCalledWith("harness-1");
  });

  it("offers a direct retry path with the recorded failure", async () => {
    const onRetry = vi.fn();
    render(
      <HarnessStatusCard
        jobs={[job({ status: "failed", error: "구조화 응답이 올바르지 않습니다." })]}
        pendingJobId={null}
        onRuntimeConfirm={vi.fn()}
        onSkip={vi.fn()}
        onRetry={onRetry}
        onDismiss={vi.fn()}
        onDecide={vi.fn()}
      />,
    );

    expect(screen.getByText("구조화 응답이 올바르지 않습니다.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(onRetry).toHaveBeenCalledWith("harness-1");
  });

  it("reviews the generated documents as one atomic secondary changeset", async () => {
    const onDecide = vi.fn();
    render(
      <HarnessStatusCard
        jobs={[
          job({
            status: "review",
            runtimeVerification: "confirmed",
            attempts: 1,
            changeset: {
              request_id: "req-harness",
              items: [
                {
                  category: "file",
                  id: "workspace-1",
                  seq: 1,
                  path: "specs/weapons.md",
                  change: "modified",
                  diff: "@@ -1 +1 @@\n-old\n+new",
                },
              ],
            },
          }),
        ]}
        pendingJobId={null}
        onRuntimeConfirm={vi.fn()}
        onSkip={vi.fn()}
        onRetry={vi.fn()}
        onDismiss={vi.fn()}
        onDecide={onDecide}
      />,
    );

    expect(screen.getByText("일괄 검토")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "적용 유지" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "전체 적용 유지" }));
    expect(onDecide).toHaveBeenCalledWith("harness-1", "accept");
  });

  it.each(["failed", "rejected"] as const)(
    "allows a terminal %s job to be dismissed",
    async (status) => {
      const onDismiss = vi.fn();
      render(
        <HarnessStatusCard
          jobs={[job({ status, error: status === "failed" ? "실패" : undefined })]}
          pendingJobId={null}
          onRuntimeConfirm={vi.fn()}
          onSkip={vi.fn()}
          onRetry={vi.fn()}
          onDismiss={onDismiss}
          onDecide={vi.fn()}
        />,
      );

      await userEvent.click(screen.getByRole("button", { name: "하네스 상태 닫기" }));
      expect(onDismiss).toHaveBeenCalledWith("harness-1");
    },
  );

  it.each(["completed", "skipped"] as const)(
    "automatically closes a terminal %s job",
    (status) => {
      const onDismiss = vi.fn();
      render(
        <HarnessStatusCard
          jobs={[job({ status })]}
          pendingJobId={null}
          onRuntimeConfirm={vi.fn()}
          onSkip={vi.fn()}
          onRetry={vi.fn()}
          onDismiss={onDismiss}
          onDecide={vi.fn()}
        />,
      );

      expect(screen.queryByRole("region", { name: "하네스 동기화" })).not.toBeInTheDocument();
      expect(onDismiss).not.toHaveBeenCalled();
    },
  );

  it("keeps older terminal jobs hidden after the newest terminal job closes automatically", () => {
    render(
      <HarnessStatusCard
        jobs={[
          job({ id: "older", status: "rejected", updatedAt: 1 }),
          job({ id: "newest", status: "completed", updatedAt: 2 }),
        ]}
        pendingJobId={null}
        onRuntimeConfirm={vi.fn()}
        onSkip={vi.fn()}
        onRetry={vi.fn()}
        onDismiss={vi.fn()}
        onDecide={vi.fn()}
      />,
    );

    expect(screen.queryByRole("region", { name: "하네스 동기화" })).not.toBeInTheDocument();
  });
});
