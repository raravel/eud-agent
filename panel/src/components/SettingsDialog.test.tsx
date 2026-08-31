import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { AppSettings } from "@/lib/ipc";
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
    ...overrides,
  };
  return { ...render(<SettingsDialog {...props} />), props };
}

describe("SettingsDialog provider management", () => {
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
