import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { BootstrapView } from "@/setup/bootstrap";
import { SetupScreen } from "@/setup/SetupScreen";
import type {
  ProviderId,
  ProviderModel,
  ProviderStatus,
} from "@/providers/types";

const idleView: BootstrapView = {
  pct: null,
  label: "설치 준비 중…",
  phase: "downloading",
};

const statuses: ProviderStatus[] = [
  {
    provider: "codex",
    availability: "ready",
    selectedAsDefault: true,
    canInstall: true,
    canImport: true,
    experimental: false,
  },
  {
    provider: "claude-code",
    availability: "needs-authentication",
    selectedAsDefault: false,
    canInstall: true,
    canImport: true,
    experimental: false,
  },
  {
    provider: "antigravity",
    availability: "needs-authentication",
    selectedAsDefault: false,
    canInstall: false,
    canImport: false,
    experimental: true,
    detailCode: "provider_credential_missing",
  },
  {
    provider: "opencode-go",
    availability: "needs-credential",
    selectedAsDefault: false,
    canInstall: false,
    canImport: false,
    experimental: false,
  },
  {
    provider: "ollama",
    availability: "unavailable",
    selectedAsDefault: false,
    canInstall: false,
    canImport: false,
    experimental: false,
    detailCode: "provider_transport_closed",
  },
];

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

const noop = vi.fn(async () => {});

function renderScreen(
  overrides: Partial<Parameters<typeof SetupScreen>[0]> = {},
) {
  return render(
    <SetupScreen
      editorValid
      pickError={null}
      onPick={vi.fn()}
      view={idleView}
      error={null}
      onRetry={vi.fn()}
      assetsReady
      providers={statuses}
      defaultProvider="codex"
      models={{ codex: [codexModel] }}
      selectedModels={{ codex: "gpt-test" }}
      selectedReasoning={{ codex: { level: "medium" } }}
      providerErrors={{}}
      onSelectProvider={noop}
      onProviderInstall={noop}
      onProviderLogin={noop}
      onProviderLoginCancel={noop}
      onProviderImport={noop}
      onProviderApiKey={noop}
      onProviderBaseUrl={noop}
      onProviderLogout={noop}
      onProviderRefresh={noop}
      onProviderModelChange={noop}
      {...overrides}
    />,
  );
}

describe("SetupScreen five-provider gate", () => {
  it("shows the editor picker before assets or providers", async () => {
    const onPick = vi.fn();
    renderScreen({ editorValid: false, assetsReady: false, onPick });
    await userEvent.click(screen.getByRole("button", { name: "폴더 선택" }));
    expect(onPick).toHaveBeenCalledOnce();
    expect(
      screen.queryByRole("heading", { name: "사용할 AI 제공자 선택" }),
    ).not.toBeInTheDocument();
  });

  it("renders determinate asset progress as step two", () => {
    renderScreen({
      assetsReady: false,
      view: { pct: 45, label: "문서 인덱스 다운로드", phase: "downloading" },
    });
    expect(screen.getByRole("progressbar", { name: "에셋 다운로드" })).toHaveAttribute(
      "aria-valuenow",
      "45",
    );
    expect(screen.getByText("문서 인덱스 다운로드")).toBeInTheDocument();
  });

  it("renders one large provider select and only the selected connection panel", () => {
    renderScreen();
    const select = screen.getByRole("combobox", {
      name: "기본 AI 제공자 선택",
    });
    expect(select).toHaveValue("codex");
    expect(Array.from((select as HTMLSelectElement).options)).toHaveLength(6);
    expect(screen.getByRole("heading", { name: "Codex" })).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Claude Code" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByText(/Google의 공개 안정 API가 아니며/)).not.toBeInTheDocument();
  });

  it("switches the single connection panel after selecting an unconnected provider", async () => {
    const onSelectProvider = vi.fn(async (_provider: ProviderId) => {});
    renderScreen({ onSelectProvider });
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "기본 AI 제공자 선택" }),
      "claude-code",
    );
    expect(onSelectProvider).toHaveBeenCalledWith("claude-code");
    expect(
      screen.getByRole("heading", { name: "Claude Code" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Codex" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Claude 로그인" })).toBeInTheDocument();
  });

  it("opens Google login when Antigravity is selected", async () => {
    const onProviderLogin = vi.fn(async () => {});
    renderScreen({ onProviderLogin });
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "기본 AI 제공자 선택" }),
      "antigravity",
    );
    const login = screen.getByRole("button", { name: "Google 로그인" });
    await userEvent.click(login);
    expect(onProviderLogin).toHaveBeenCalledWith("antigravity");
    expect(screen.queryByText(/Google의 공개 안정 API가 아니며/)).not.toBeInTheDocument();
  });

  it("clears the selected OpenCode Go API key field after submission", async () => {
    const onProviderApiKey = vi.fn(async () => {});
    renderScreen({
      defaultProvider: "opencode-go",
      selectedModels: {},
      selectedReasoning: {},
      onProviderApiKey,
    });
    const input = screen.getByPlaceholderText("API 키");
    await userEvent.type(input, "secret-key");
    await userEvent.click(screen.getByRole("button", { name: "연결" }));
    expect(onProviderApiKey).toHaveBeenCalledWith("opencode-go", "secret-key");
    await waitFor(() => expect(input).toHaveValue(""));
  });
});
