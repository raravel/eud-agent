/**
 * First-run setup overlay (EUD-120, EUD-132).
 *
 * The setup screen is a full-screen dialog with two steps. The pick step is
 * shown while the editor path is missing/invalid and drives the native folder
 * picker through the backend. The download step renders Korean setup text and
 * an accessible progressbar while bootstrap progress is active; error mode
 * renders the bootstrap error and a retry control, with no progress bar.
 *
 * Contract:
 *   export interface SetupScreenProps {
 *     editorValid: boolean;
 *     pickError: string | null;
 *     onPick: () => void;
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

const idleView: BootstrapView = {
  pct: null,
  label: "설치 준비 중…",
  phase: "downloading",
};

function renderScreen(overrides: Partial<Parameters<typeof SetupScreen>[0]>) {
  return render(
    <SetupScreen
      editorValid={true}
      pickError={null}
      onPick={vi.fn()}
      view={idleView}
      error={null}
      onRetry={vi.fn()}
      {...overrides}
    />,
  );
}

describe("SetupScreen", () => {
  it("renders determinate setup progress with the visible label", () => {
    const view: BootstrapView = {
      pct: 45,
      label: "bge-m3 모델 다운로드 45%",
      phase: "downloading",
    };

    renderScreen({ view });

    expect(
      screen.getByRole("dialog", { name: "최초 실행 설정" }),
    ).toBeInTheDocument();
    const progress = screen.getByRole("progressbar");
    expect(progress).toHaveAttribute("aria-valuenow", "45");
    expect(screen.getByText("bge-m3 모델 다운로드 45%")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
  });

  it("renders indeterminate setup progress without aria-valuenow", () => {
    renderScreen({ view: idleView });

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

    renderScreen({ view, error: "디스크 공간 부족", onRetry });

    expect(screen.getByText("디스크 공간 부족")).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("shows the editor-folder pick step before anything downloads", () => {
    const onPick = vi.fn();

    renderScreen({ editorValid: false, onPick });

    expect(
      screen.getByText("EUD Editor 3 설치 폴더를 선택해 주세요."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "에디터 폴더 선택" }));
    expect(onPick).toHaveBeenCalledTimes(1);
  });

  it("maps the invalid_editor_folder code to Korean text, never raw", () => {
    renderScreen({ editorValid: false, pickError: "invalid_editor_folder" });

    expect(
      screen.getByText(/Data\\Lua\\TriggerEditor 폴더가 있는 설치 폴더/),
    ).toBeInTheDocument();
    expect(screen.queryByText("invalid_editor_folder")).not.toBeInTheDocument();
  });

  it("marks step 1 current while picking and step 2 current while downloading", () => {
    const { unmount } = renderScreen({ editorValid: false });
    expect(
      screen.getByText("에디터 폴더").closest("li"),
    ).toHaveAttribute("aria-current", "step");
    expect(
      screen.getByText("에셋 다운로드").closest("li"),
    ).not.toHaveAttribute("aria-current");
    unmount();

    renderScreen({ editorValid: true });
    expect(
      screen.getByText("에셋 다운로드").closest("li"),
    ).toHaveAttribute("aria-current", "step");
    expect(
      screen.getByText("에디터 폴더").closest("li"),
    ).not.toHaveAttribute("aria-current");
  });

  it("prefers the pick step over download UI while the path is invalid", () => {
    // A stale bootstrap error must not hide the picker (the pick step is the
    // prerequisite; retry without a valid path would fail again).
    renderScreen({ editorValid: false, error: "디스크 공간 부족" });

    expect(
      screen.getByRole("button", { name: "에디터 폴더 선택" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
  });
});
