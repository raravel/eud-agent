import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { AppSettings, EuddraftSettings } from "@/lib/ipc";
import type { ProviderModel, ProviderStatus } from "@/providers/types";
import { SettingsDialog } from "./SettingsDialog";

const settings: AppSettings = {
  notifications: {
    planApproval: { sound: true, osNotification: true },
    changesetReview: { sound: true, osNotification: true },
    agentTurnComplete: { sound: true, osNotification: true },
    askResponseRequired: { sound: true, osNotification: true },
  },
  codexLargeContextModels: [],
  deepPlanning: false,
  scmdraftPath: "",
};

const euddraft: EuddraftSettings = {
  path: String.raw`C:\Users\tester\AppData\Local\eud-agent\euddraft\old\euddraft.exe`,
  valid: true,
  managed: true,
  installedVersion: "v0.10.2.5",
  latestVersion: "v0.11.0.1",
  updateAvailable: true,
};

const providers: ProviderStatus[] = [
  ["codex", true, false],
  ["claude-code", false, false],
  ["antigravity", false, true],
  ["opencode-go", false, false],
  ["ollama", false, false],
].map(([provider, selectedAsDefault, experimental]) => ({
  provider: provider as ProviderStatus["provider"],
  availability: "ready" as const,
  selectedAsDefault: Boolean(selectedAsDefault),
  canInstall: provider === "codex" || provider === "claude-code",
  canImport: provider === "codex" || provider === "claude-code",
  experimental: Boolean(experimental),
}));

const codexModel: ProviderModel = {
  provider: "codex",
  model: "gpt-test",
  displayName: "GPT Test",
  description: "test",
  isDefault: true,
  capabilities: {
    vision: true,
    toolCalls: true,
    strictStructuredOutput: true,
    reasoningLevels: ["medium", "high"],
    nativeCompaction: true,
    hostedWebSearch: true,
  },
};

function renderDialog(
  overrides: Partial<Parameters<typeof SettingsDialog>[0]> = {},
) {
  const props: Parameters<typeof SettingsDialog>[0] = {
    open: true,
    settings,
    providers,
    providerModels: { codex: [codexModel] },
    selectedModels: { codex: "gpt-test" },
    selectedReasoning: { codex: { level: "medium" } },
    providerErrors: {},
    euddraft,
    onOpenChange: vi.fn(),
    onSettingsChange: vi.fn(),
    onReload: vi.fn(),
    onPreviewSound: vi.fn(),
    onSelectProvider: vi.fn(),
    onProviderInstall: vi.fn(),
    onProviderLogin: vi.fn(),
    onProviderLoginCancel: vi.fn(),
    onProviderImport: vi.fn(),
    onProviderApiKey: vi.fn(),
    onProviderBaseUrl: vi.fn(),
    onProviderLogout: vi.fn(),
    onProviderRefresh: vi.fn(),
    onProviderModelChange: vi.fn(),
    onProjectOpen: vi.fn(),
    onProjectCreate: vi.fn(),
    onProjectImport: vi.fn(),
    onProjectExport: vi.fn(),
    onEuddraftCheck: vi.fn(),
    onEuddraftUpdate: vi.fn(),
    onScmdraftPick: vi.fn(),
    ...overrides,
  };
  return { ...render(<SettingsDialog {...props} />), props };
}

describe("SettingsDialog provider management", () => {
  it("exposes native project and E3S actions", async () => {
    const onProjectOpen = vi.fn();
    const onProjectCreate = vi.fn();
    const onProjectImport = vi.fn();
    const onProjectExport = vi.fn();
    renderDialog({
      onProjectOpen,
      onProjectCreate,
      onProjectImport,
      onProjectExport,
    });

    await userEvent.click(screen.getByRole("button", { name: "프로젝트" }));
    await userEvent.click(
      screen.getByRole("button", { name: "기존 Native 프로젝트 열기" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "새 프로젝트" }));
    await userEvent.click(screen.getByRole("button", { name: "E3S 가져오기" }));
    await userEvent.click(screen.getByRole("button", { name: "E3S 내보내기" }));

    expect(onProjectOpen).toHaveBeenCalledOnce();
    expect(onProjectCreate).toHaveBeenCalledOnce();
    expect(onProjectImport).toHaveBeenCalledOnce();
    expect(onProjectExport).toHaveBeenCalledOnce();
  });

  it("shows the SCMDraft 2 executable under 컴파일 and lets the user pick it", async () => {
    const onScmdraftPick = vi.fn();
    const { rerender, props } = renderDialog({ onScmdraftPick });

    await userEvent.click(screen.getByRole("button", { name: "컴파일" }));
    const section = screen.getByRole("region", { name: "SCMDraft 2" });
    expect(within(section).getByText("지정되지 않음")).toBeInTheDocument();
    await userEvent.click(within(section).getByRole("button", { name: "실행 파일 선택" }));
    expect(onScmdraftPick).toHaveBeenCalledOnce();

    const scmdraftPath = String.raw`C:\Tools\ScmDraft 2\ScmDraft 2.exe`;
    rerender(
      <SettingsDialog
        {...props}
        settings={{ ...settings, scmdraftPath }}
        scmdraftBusy
        scmdraftError="SCMDraft 2 실행 파일을 설정하지 못했습니다. ScmDraft 2.exe를 다시 선택해 주세요."
      />,
    );
    const updated = screen.getByRole("region", { name: "SCMDraft 2" });
    expect(within(updated).getByText(scmdraftPath)).toBeInTheDocument();
    expect(within(updated).getByRole("button", { name: "선택 중…" })).toBeDisabled();
    expect(within(updated).getByRole("alert")).toHaveTextContent("다시 선택해 주세요");
  });

  it("shows managed euddraft versions and exposes check and update actions", async () => {
    const onEuddraftCheck = vi.fn();
    const onEuddraftUpdate = vi.fn();
    renderDialog({ onEuddraftCheck, onEuddraftUpdate });

    await userEvent.click(screen.getByRole("button", { name: "컴파일" }));

    expect(screen.getByText(euddraft.path)).toBeInTheDocument();
    expect(screen.getByText("v0.10.2.5")).toBeInTheDocument();
    expect(screen.getAllByText("v0.11.0.1")).not.toHaveLength(0);
    expect(
      screen.getByText(/새 euddraft v0\.11\.0\.1 버전을 설치할 수 있습니다/),
    ).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: "최신 버전 확인" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "최신 버전으로 업데이트" }),
    );
    expect(onEuddraftCheck).toHaveBeenCalledOnce();
    expect(onEuddraftUpdate).toHaveBeenCalledOnce();
  });

  it("shows provider status summaries before opening one provider at a time", async () => {
    renderDialog({
      providers: providers.map((status) =>
        status.provider === "claude-code"
          ? { ...status, availability: "needs-authentication" }
          : status,
      ),
    });

    expect(
      screen.getByText(
        /기존 EPS·Map 세션과 하네스 작업의 제공자는 바뀌지 않습니다/,
      ),
    ).toBeInTheDocument();

    const list = screen.getByRole("list", { name: "AI 제공자 목록" });
    for (const name of [
      "Codex",
      "Claude Code",
      "Antigravity",
      "OpenCode Go",
      "Ollama",
    ]) {
      expect(
        within(list).getByRole("button", {
          name: new RegExp(`^${name} 설정 열기`),
        }),
      ).toBeInTheDocument();
    }
    expect(
      within(
        screen.getByRole("button", {
          name: /^Claude Code 설정 열기/,
        }),
      ).getByText("로그인 필요"),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Codex" }),
    ).not.toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /^Codex 설정 열기/ }),
    );

    expect(
      screen.getByRole("heading", { name: "Codex 설정" }),
    ).toHaveFocus();
    expect(screen.getByRole("heading", { name: "Codex" })).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^Claude Code 설정 열기/ }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "설정 완료" }));

    expect(
      screen.getByRole("button", { name: /^Codex 설정 열기/ }),
    ).toHaveFocus();
  });

  it("persists Codex 1M context independently from provider defaults", async () => {
    const onSettingsChange = vi.fn();
    renderDialog({ onSettingsChange });
    await userEvent.click(
      screen.getByRole("button", { name: /^Codex 설정 열기/ }),
    );
    await userEvent.click(
      screen.getByRole("switch", { name: "GPT Test 1M 컨텍스트" }),
    );
    expect(onSettingsChange).toHaveBeenCalledWith({
      ...settings,
      codexLargeContextModels: ["gpt-test"],
    });
  });

  it("round-trips the deep-planning switch under the AI provider section", async () => {
    const onSettingsChange = vi.fn();
    const { rerender, props } = renderDialog({ onSettingsChange });
    const toggle = screen.getByRole("switch", { name: "더 똑똑한 계획" });
    expect(toggle).not.toBeChecked();
    expect(
      screen.getByText(
        "계획마다 모델 호출이 여러 번 추가되어 토큰 사용량이 크게 늘어납니다.",
      ),
    ).toBeInTheDocument();
    await userEvent.click(toggle);
    expect(onSettingsChange).toHaveBeenCalledWith({
      ...settings,
      deepPlanning: true,
    });

    rerender(
      <SettingsDialog {...props} settings={{ ...settings, deepPlanning: true }} />,
    );
    const enabled = screen.getByRole("switch", { name: "더 똑똑한 계획" });
    expect(enabled).toBeChecked();
    await userEvent.click(enabled);
    expect(onSettingsChange).toHaveBeenLastCalledWith({
      ...settings,
      deepPlanning: false,
    });
  });

  it("keeps notification controls available in their own category", async () => {
    const onSettingsChange = vi.fn();
    renderDialog({ onSettingsChange });
    await userEvent.click(screen.getByRole("button", { name: "알림" }));
    await userEvent.click(screen.getByRole("switch", { name: "에이전트 턴 종료 알림음" }));
    expect(onSettingsChange).toHaveBeenCalledWith({
      ...settings,
      notifications: {
        ...settings.notifications,
        agentTurnComplete: {
          ...settings.notifications.agentTurnComplete,
          sound: false,
        },
      },
    });
  });
});
