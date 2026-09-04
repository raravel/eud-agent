/**
 * Native project/euddraft/assets/codex first-run setup overlay.
 *
 * The setup screen is a full-screen dialog with four ordered prerequisites.
 * Project and euddraft selection are explicit native pick steps. Once both
 * paths are valid, bootstrap progress and codex authentication retain their
 * accessible progress/error controls.
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
      projectValid={true}
      euddraftValid={true}
      pickError={null}
      onPickProject={vi.fn()}
      onCreateProject={vi.fn()}
      onImportE3s={vi.fn()}
      onPickEuddraft={vi.fn()}
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

  it("shows the native-project pick step before anything downloads", () => {
    const onPickProject = vi.fn();

    renderScreen({ projectValid: false, onPickProject });

    expect(
      screen.getByText("Native EUD 프로젝트 폴더를 선택해 주세요."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "기존 Native 프로젝트 열기" }));
    expect(onPickProject).toHaveBeenCalledTimes(1);
  });

  it("offers create and E3S import as explicit secondary project actions", () => {
    const onCreateProject = vi.fn();
    const onImportE3s = vi.fn();
    renderScreen({ projectValid: false, onCreateProject, onImportE3s });

    fireEvent.click(screen.getByRole("button", { name: "새 프로젝트 만들기" }));
    fireEvent.click(screen.getByRole("button", { name: "E3S 가져오기" }));
    expect(onCreateProject).toHaveBeenCalledTimes(1);
    expect(onImportE3s).toHaveBeenCalledTimes(1);
  });

  it("shows the euddraft pick step after a project is selected", () => {
    const onPickEuddraft = vi.fn();

    renderScreen({ euddraftValid: false, onPickEuddraft });

    expect(
      screen.getByText("euddraft 실행 파일을 선택해 주세요."),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "euddraft 선택" }));
    expect(onPickEuddraft).toHaveBeenCalledTimes(1);
  });

  it("maps native picker error codes to Korean text, never raw", () => {
    renderScreen({
      projectValid: false,
      pickError: "invalid_project_folder",
    });

    expect(
      screen.getByText(/project\.json이 있는 Native EUD 프로젝트 폴더/),
    ).toBeInTheDocument();
    expect(screen.queryByText("invalid_project_folder")).not.toBeInTheDocument();
  });

  it("marks each native prerequisite as current in order", () => {
    const first = renderScreen({ projectValid: false });
    expect(screen.getByText("프로젝트").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    first.unmount();

    const second = renderScreen({ euddraftValid: false });
    expect(screen.getByText("euddraft").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
    second.unmount();

    renderScreen({});
    expect(screen.getByText("에셋").closest("li")).toHaveAttribute(
      "aria-current",
      "step",
    );
  });

  it("prefers project selection over a stale bootstrap error", () => {
    renderScreen({ projectValid: false, error: "디스크 공간 부족" });

    expect(
      screen.getByRole("button", { name: "기존 Native 프로젝트 열기" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "다시 시도" })).not.toBeInTheDocument();
  });
});

// ---- step 4: codex login (native paths + assets done, codex not yet authed) -
describe("SetupScreen — codex login step", () => {
  const codexProps = {
    projectValid: true,
    euddraftValid: true,
    assetsReady: true,
    codexResolved: true,
    codexAuthed: false,
  };

  it("shows the codex login step with both auth paths once assets are ready", () => {
    renderScreen(codexProps);

    expect(
      screen.getByRole("button", { name: "ChatGPT로 로그인" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("OpenAI API 키")).toBeInTheDocument();
    // The download progressbar must NOT be shown (assets are done).
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    // Step 3 is the current step.
    expect(
      screen.getByText("codex").closest("li"),
    ).toHaveAttribute("aria-current", "step");
  });

  it("launches OAuth and submits the API key through the callbacks", () => {
    const onCodexOAuth = vi.fn();
    const onCodexApiKey = vi.fn();
    renderScreen({ ...codexProps, onCodexOAuth, onCodexApiKey });

    fireEvent.click(screen.getByRole("button", { name: "ChatGPT로 로그인" }));
    expect(onCodexOAuth).toHaveBeenCalledTimes(1);

    fireEvent.change(screen.getByLabelText("OpenAI API 키"), {
      target: { value: "  sk-test-key  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "API 키로 로그인" }));
    // The key is trimmed before it reaches the backend (stdin-only contract).
    expect(onCodexApiKey).toHaveBeenCalledWith("sk-test-key");
  });

  it("offers an install button (not manual guidance) when codex is not found", () => {
    const onCodexInstall = vi.fn();
    renderScreen({ ...codexProps, codexResolved: false, onCodexInstall });

    const install = screen.getByRole("button", { name: "codex 설치" });
    expect(install).toBeInTheDocument();
    // The login controls are hidden until codex is installed/resolved.
    expect(
      screen.queryByRole("button", { name: "ChatGPT로 로그인" }),
    ).not.toBeInTheDocument();

    fireEvent.click(install);
    expect(onCodexInstall).toHaveBeenCalledTimes(1);
  });

  it("shows an installing spinner while the codex download is in flight", () => {
    renderScreen({ ...codexProps, codexResolved: false, codexBusy: true });

    expect(
      screen.getByRole("button", { name: "codex 설치 중…" }),
    ).toBeDisabled();
  });

  it("disables the login controls while a login attempt is in flight", () => {
    renderScreen({ ...codexProps, codexBusy: true });

    expect(
      screen.getByRole("button", { name: "로그인 진행 중…" }),
    ).toBeDisabled();
  });
});
