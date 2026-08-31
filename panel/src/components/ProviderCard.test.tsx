import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { ProviderCard } from "./ProviderCard";
import { providerErrorCopy } from "@/providers/providerCopy";
import type { ProviderStatus } from "@/providers/types";

const noop = vi.fn(async () => {});

function renderCard(
  status: ProviderStatus,
  overrides: Partial<Parameters<typeof ProviderCard>[0]> = {},
) {
  return render(
    <ProviderCard
      status={status}
      selected={status.selectedAsDefault}
      onSelectDefault={noop}
      onInstall={noop}
      onLogin={noop}
      onLoginCancel={noop}
      onImport={noop}
      onApiKey={noop}
      onBaseUrl={noop}
      onLogout={noop}
      onRefresh={noop}
      onModelChange={noop}
      {...overrides}
    />,
  );
}

describe("ProviderCard", () => {
  it("clears an OpenCode Go key and never echoes it after submit", async () => {
    const onApiKey = vi.fn(async () => {});
    renderCard(
      {
        provider: "opencode-go",
        availability: "needs-credential",
        selectedAsDefault: false,
        canInstall: false,
        canImport: false,
        experimental: false,
      },
      { onApiKey },
    );
    const input = screen.getByLabelText("API 키");
    await userEvent.type(input, "private-key");
    await userEvent.click(screen.getByRole("button", { name: "연결" }));
    expect(onApiKey).toHaveBeenCalledWith("opencode-go", "private-key");
    await waitFor(() => expect(input).toHaveValue(""));
    expect(screen.queryByText("private-key")).not.toBeInTheDocument();
  });

  it("saves an Ollama endpoint, direct model, reasoning, and optional key", async () => {
    const onBaseUrl = vi.fn(async () => {});
    const onModelChange = vi.fn(async () => {});
    const onApiKey = vi.fn(async () => {});
    const user = userEvent.setup();
    renderCard(
      {
        provider: "ollama",
        availability: "ready",
        selectedAsDefault: true,
        canInstall: false,
        canImport: false,
        experimental: false,
      },
      {
        baseUrl: "http://localhost:11434/v1",
        onBaseUrl,
        onModelChange,
        selectedReasoning: { level: "high" },
        onApiKey,
      },
    );

    const endpoint = screen.getByLabelText("OpenAI 호환 Base URL");
    await user.clear(endpoint);
    await user.type(endpoint, "https://ollama.example.test/v1");
    await user.click(screen.getByRole("button", { name: "URL 저장" }));
    expect(onBaseUrl).toHaveBeenCalledWith(
      "ollama",
      "https://ollama.example.test/v1",
    );

    await user.type(screen.getByLabelText("기본 모델"), "qwen3:8b");
    await user.click(screen.getByRole("button", { name: "모델 저장" }));
    expect(onModelChange).toHaveBeenCalledWith("ollama", "qwen3:8b", {
      level: "high",
    });

    const key = screen.getByLabelText("선택적 API 키");
    await user.type(key, "proxy-key");
    await user.click(screen.getByRole("button", { name: "키 저장" }));
    expect(onApiKey).toHaveBeenCalledWith("ollama", "proxy-key");
    await waitFor(() => expect(key).toHaveValue(""));
  });

  it("selects a ready provider as the new default", async () => {
    const onSelectDefault = vi.fn(async () => {});
    renderCard(
      {
        provider: "codex",
        availability: "ready",
        selectedAsDefault: false,
        canInstall: true,
        canImport: true,
        experimental: false,
      },
      { onSelectDefault },
    );

    const radio = screen.getByRole("radio", { name: /기본 제공자/ });
    expect(radio).not.toBeChecked();

    await userEvent.click(radio);

    expect(onSelectDefault).toHaveBeenCalledWith("codex");
  });

  it("omits reasoning when the selected model exposes no reasoning capability", () => {
    renderCard(
      {
        provider: "claude-code",
        availability: "ready",
        selectedAsDefault: true,
        canInstall: true,
        canImport: true,
        experimental: false,
      },
      {
        models: [
          {
            provider: "claude-code",
            model: "provider-default",
            displayName: "Provider Managed Default",
            description: "fast",
            isDefault: true,
            capabilities: {
              vision: true,
              toolCalls: true,
              strictStructuredOutput: true,
              reasoningLevels: [],
              nativeCompaction: true,
              hostedWebSearch: false,
            },
          },
        ],
        selectedModel: "provider-default",
      },
    );
    expect(screen.queryByText("추론 강도")).not.toBeInTheDocument();
  });

  it("maps backend status codes to product copy instead of rendering raw identifiers", () => {
    renderCard({
      provider: "antigravity",
      availability: "unavailable",
      selectedAsDefault: false,
      canInstall: false,
      canImport: false,
      experimental: true,
      detailCode: "provider_protocol_changed",
    });
    expect(screen.getByText(/응답 형식이 변경/)).toBeInTheDocument();
    expect(screen.queryByText("provider_protocol_changed")).not.toBeInTheDocument();
  });

  it("keeps an active login cancellable while every other action is disabled", async () => {
    const onLoginCancel = vi.fn(async () => {});
    renderCard(
      {
        provider: "antigravity",
        availability: "needs-authentication",
        selectedAsDefault: false,
        canInstall: false,
        canImport: false,
        experimental: true,
      },
      { busy: true, loginInProgress: true, onLoginCancel },
    );
    expect(screen.queryByRole("button", { name: "Google 로그인" })).not.toBeInTheDocument();
    expect(screen.getByText("로그인 대기 중…")).toBeInTheDocument();
    const cancel = screen.getByRole("button", { name: "로그인 취소" });
    expect(cancel).toBeEnabled();
    await userEvent.click(cancel);
    expect(onLoginCancel).toHaveBeenCalledWith("antigravity");
  });

  it("uses Google-specific recovery copy for Antigravity token exchange failures", () => {
    renderCard({
      provider: "antigravity",
      availability: "needs-authentication",
      selectedAsDefault: false,
      canInstall: false,
      canImport: false,
      experimental: true,
      detailCode: "provider_oauth_exchange_failed",
    });
    expect(screen.getByText(/Google 로그인 결과를 확인하지 못했습니다/)).toBeInTheDocument();
    expect(screen.queryByText("provider_oauth_exchange_failed")).not.toBeInTheDocument();
  });

  it.each([
    [
      "provider_oauth_client_unconfigured",
      /Antigravity OAuth client가 설정되지 않았습니다/,
    ],
    [
      "provider_cloud_code_unauthorized",
      /Cloud Code Assist가 요청을 거부했습니다/,
    ],
    ["provider_account_ineligible", /Antigravity를 사용할 수 없습니다/],
    ["provider_onboarding_required", /초기 설정을 완료하지 못했습니다/],
  ] as const)("shows a distinct Antigravity recovery for %s", (detailCode, copy) => {
    renderCard({
      provider: "antigravity",
      availability: "unavailable",
      selectedAsDefault: false,
      canInstall: false,
      canImport: false,
      experimental: true,
      detailCode,
    });
    expect(screen.getByText(copy)).toBeInTheDocument();
    expect(screen.queryByText(detailCode)).not.toBeInTheDocument();
  });

  it("recovers stable codes from wrapped Tauri and JavaScript errors", () => {
    expect(
      providerErrorCopy(
        'Error: IPC command failed: "provider_cloud_code_unauthorized"',
        "antigravity",
      ),
    ).toMatch(/Cloud Code Assist가 요청을 거부했습니다/);
    expect(
      providerErrorCopy("Error: invalid provider settings response", "antigravity"),
    ).toMatch(/응답 형식이 변경/);
    expect(
      providerErrorCopy("unexpected Antigravity failure", "antigravity"),
    ).toMatch(/응답 형식이 변경/);
    expect(
      providerErrorCopy(
        "provider_credential_store_unavailable",
        "antigravity",
      ),
    ).toMatch(/Windows 자격 증명 저장소/);
    expect(
      providerErrorCopy("provider_endpoint_invalid", "ollama"),
    ).toMatch(/Base URL/);
  });

  it("renders arbitrary Antigravity models supplied by the live catalog", async () => {
    const capabilities = {
      vision: true,
      toolCalls: true,
      strictStructuredOutput: true,
      reasoningLevels: [],
      nativeCompaction: false,
      hostedWebSearch: false,
    };
    renderCard(
      {
        provider: "antigravity",
        availability: "ready",
        selectedAsDefault: true,
        canInstall: false,
        canImport: false,
        experimental: true,
      },
      {
        models: [
          {
            provider: "antigravity",
            model: "provider-future-thinking-v9",
            displayName: "Provider Future Thinking",
            description: "provider-model · provider-api",
            isDefault: true,
            capabilities,
          },
          {
            provider: "antigravity",
            model: "provider-future-fast-v2",
            displayName: "Provider Future Fast",
            description: "provider-model · provider-api",
            isDefault: false,
            capabilities,
          },
        ],
        selectedModel: "provider-future-thinking-v9",
      },
    );
    await userEvent.click(
      screen.getByRole("combobox", { name: "기본 모델" }),
    );
    expect(
      screen.getByRole("option", { name: "Provider Future Thinking" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: "Provider Future Fast" }),
    ).toBeInTheDocument();
  });
});
