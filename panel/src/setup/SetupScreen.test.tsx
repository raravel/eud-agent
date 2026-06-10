/**
 * First-run setup overlay (EUD-120).
 *
 * The setup screen is a full-screen dialog shown while bootstrap progress is
 * active. Progress mode renders Korean setup text and an accessible progressbar;
 * error mode renders the bootstrap error and a retry control, with no progress
 * bar.
 *
 * Contract (Step B implements `@/setup/SetupScreen`):
 *   export interface SetupScreenProps {
 *     view: BootstrapView;
 *     error: string | null;
 *     onRetry: () => void;
 *   }
 *   export function SetupScreen(props): JSX.Element;
 */
import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { SetupScreen } from "@/setup/SetupScreen";
import type { BootstrapView } from "@/setup/bootstrap";

describe("SetupScreen", () => {
  it("renders determinate setup progress with the visible label", () => {
    const view: BootstrapView = {
      pct: 45,
      label: "bge-m3 모델 다운로드 45%",
      phase: "downloading",
    };

    render(<SetupScreen view={view} error={null} onRetry={vi.fn()} />);

    expect(
      screen.getByRole("dialog", { name: "최초 실행 설정" }),
    ).toBeInTheDocument();
    const progress = screen.getByRole("progressbar");
    expect(progress).toHaveAttribute("aria-valuenow", "45");
    expect(screen.getByText("bge-m3 모델 다운로드 45%")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
  });

  it("renders indeterminate setup progress without aria-valuenow", () => {
    const view: BootstrapView = {
      pct: null,
      label: "설치 준비 중…",
      phase: "downloading",
    };

    render(<SetupScreen view={view} error={null} onRetry={vi.fn()} />);

    const progress = screen.getByRole("progressbar");
    expect(progress).toHaveAttribute("aria-busy", "true");
    expect(progress).not.toHaveAttribute("aria-valuenow");
  });

  it("renders an error with a retry button that calls onRetry", () => {
    const onRetry = vi.fn();
    const view: BootstrapView = {
      pct: null,
      label: "error: 네트워크 오류",
      phase: "error",
    };

    render(
      <SetupScreen view={view} error="디스크 공간 부족" onRetry={onRetry} />,
    );

    expect(screen.getByText("디스크 공간 부족")).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
