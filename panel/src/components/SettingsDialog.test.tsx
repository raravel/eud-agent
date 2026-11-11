import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import {
  jevKeyClear,
  jevSettingsGet,
  jevSettingsSave,
  type AppSettings,
  type EuddraftSettings,
  type InvokeFn,
} from "@/lib/ipc";
import type { ProviderModel, ProviderStatus } from "@/providers/types";
import { SettingsDialog, type JevSettingsActions } from "./SettingsDialog";

const settings: AppSettings = {
  notifications: {
    planApproval: { sound: true, osNotification: true },
    reviewRequired: { sound: true, osNotification: true },
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
  ["opencode-zen", false, false],
  ["ollama", false, false],
].map(([provider, selectedAsDefault, experimental]) => ({
  provider: provider as ProviderStatus["provider"],
  availability: "ready" as const,
  selectedAsDefault: Boolean(selectedAsDefault),
  canInstall: provider === "codex" || provider === "claude-code",
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
    onProviderApiKey: vi.fn(),
    onProviderBaseUrl: vi.fn(),
    onProviderLogout: vi.fn(),
    onProviderRefresh: vi.fn(),
    onProviderModelChange: vi.fn(),
    onProjectOpen: vi.fn(),
    onProjectCreate: vi.fn(),
    onProjectImport: vi.fn(),
    onProjectImportE2: vi.fn(),
    onProjectExport: vi.fn(),
    onEuddraftCheck: vi.fn(),
    onEuddraftUpdate: vi.fn(),
    onScmdraftPick: vi.fn(),
    ...overrides,
  };
  return { ...render(<SettingsDialog {...props} />), props };
}

describe("SettingsDialog provider management", () => {
  it("exposes native project, separate E3S/E2 import, and export actions", async () => {
    const onProjectOpen = vi.fn();
    const onProjectCreate = vi.fn();
    const onProjectImport = vi.fn();
    const onProjectImportE2 = vi.fn();
    const onProjectExport = vi.fn();
    renderDialog({
      onProjectOpen,
      onProjectCreate,
      onProjectImport,
      onProjectImportE2,
      onProjectExport,
    });

    await userEvent.click(screen.getByRole("button", { name: "프로젝트" }));
    await userEvent.click(
      screen.getByRole("button", { name: "기존 Native 프로젝트 열기" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "새 프로젝트" }));
    await userEvent.click(screen.getByRole("button", { name: "E3S 가져오기" }));
    await userEvent.click(screen.getByRole("button", { name: "E2 프로젝트 가져오기" }));
    await userEvent.click(screen.getByRole("button", { name: "E3S 내보내기" }));

    expect(onProjectOpen).toHaveBeenCalledOnce();
    expect(onProjectCreate).toHaveBeenCalledOnce();
    expect(onProjectImport).toHaveBeenCalledOnce();
    expect(onProjectImportE2).toHaveBeenCalledOnce();
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

  it("shows the StarCraft install folder under 컴파일 and lets the user pick it", async () => {
    const onStarcraftPick = vi.fn();
    const reason = "StarCraft 설치 폴더를 찾지 못했습니다.";
    const { rerender, props } = renderDialog({
      onStarcraftPick,
      starcraft: { available: false, path: "", reason },
    });

    await userEvent.click(screen.getByRole("button", { name: "컴파일" }));
    const section = screen.getByRole("region", { name: "StarCraft" });
    expect(within(section).getByText("찾지 못함")).toBeInTheDocument();
    expect(within(section).getByText(reason)).toBeInTheDocument();
    await userEvent.click(within(section).getByRole("button", { name: "폴더 선택" }));
    expect(onStarcraftPick).toHaveBeenCalledOnce();

    const starcraftPath = String.raw`D:\Games\StarCraft`;
    rerender(
      <SettingsDialog
        {...props}
        starcraft={{ available: true, path: starcraftPath }}
        starcraftBusy
      />,
    );
    const updated = screen.getByRole("region", { name: "StarCraft" });
    expect(within(updated).getByText(starcraftPath)).toBeInTheDocument();
    expect(within(updated).getByRole("button", { name: "선택 중…" })).toBeDisabled();
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
      "OpenCode Zen",
      "OpenAI 호환",
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
    // The changeset-review channel is gone with request review; the remaining
    // three are the only rows.
    expect(
      screen.queryByRole("switch", { name: "변경사항 검토 필요 알림음" }),
    ).not.toBeInTheDocument();
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

describe("SettingsDialog browser", () => {
  it("shows the 브라우저 category only with its actions and resets through them", async () => {
    const { unmount } = renderDialog();
    expect(screen.queryByRole("button", { name: "브라우저" })).not.toBeInTheDocument();
    unmount();

    const resetProfile = vi.fn().mockResolvedValue(undefined);
    renderDialog({ browser: { resetProfile } });
    await userEvent.click(screen.getByRole("button", { name: "브라우저" }));
    expect(screen.getByRole("heading", { name: "내장 브라우저" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "로그인 데이터 초기화" }));
    await userEvent.click(await screen.findByRole("button", { name: "초기화" }));
    expect(resetProfile).toHaveBeenCalledTimes(1);
  });
});

describe("SettingsDialog Jev search rerank", () => {
  function jevActions(invoke: InvokeFn): JevSettingsActions {
    return {
      load: () => jevSettingsGet(invoke),
      save: (enabled, apiKey) => jevSettingsSave(enabled, apiKey, invoke),
      clearKey: () => jevKeyClear(invoke),
    };
  }

  it("shows the key input only when on and saves a new key through IPC", async () => {
    const invoke = vi.fn<InvokeFn>(async (command) => {
      if (command === "jev_settings") return { enabled: false, hasKey: false };
      if (command === "jev_settings_save") return { enabled: true, hasKey: true };
      throw new Error(`unexpected ${command}`);
    });
    renderDialog({ jev: jevActions(invoke) });

    expect(
      screen.getByText("켜면 검색어와 문서 조각이 TypeSafe 서버로 전송됩니다."),
    ).toBeInTheDocument();
    const toggle = await screen.findByRole("switch", {
      name: "Jev 문서 검색 재정렬",
    });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(screen.queryByLabelText("TypeSafe API 키")).not.toBeInTheDocument();

    await userEvent.click(toggle);
    const input = screen.getByLabelText("TypeSafe API 키");
    expect(input).toHaveAttribute("type", "password");
    expect(input).toHaveAttribute("placeholder", "API 키 입력");
    const save = screen.getByRole("button", { name: "저장" });
    expect(save).toBeDisabled();
    expect(invoke).not.toHaveBeenCalledWith("jev_settings_save", expect.anything());

    await userEvent.type(input, "  tsk-test  ");
    await userEvent.click(save);
    expect(invoke).toHaveBeenCalledWith("jev_settings_save", {
      enabled: true,
      apiKey: "tsk-test",
    });
    await waitFor(() =>
      expect(screen.getByLabelText("TypeSafe API 키")).toHaveAttribute(
        "placeholder",
        "저장된 키 사용 중 · 교체하려면 입력",
      ),
    );
    expect(screen.getByLabelText("TypeSafe API 키")).toHaveValue("");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it.each([
    ["jev_key_invalid", "API 키가 올바르지 않습니다. 키를 확인해 주세요."],
    ["jev_unreachable", "TypeSafe 서버에 연결할 수 없습니다. 네트워크를 확인해 주세요."],
    ["jev_key_missing", "켜려면 API 키를 입력해 주세요."],
    ["something_else", "Jev 설정을 저장하지 못했습니다."],
  ])("renders the %s save error in Korean", async (code, message) => {
    const invoke = vi.fn<InvokeFn>(async (command) => {
      if (command === "jev_settings") return { enabled: false, hasKey: false };
      throw code;
    });
    renderDialog({ jev: jevActions(invoke) });
    const toggle = await screen.findByRole("switch", {
      name: "Jev 문서 검색 재정렬",
    });
    await waitFor(() => expect(toggle).toBeEnabled());
    await userEvent.click(toggle);
    await userEvent.type(screen.getByLabelText("TypeSafe API 키"), "bad-key");
    await userEvent.click(screen.getByRole("button", { name: "저장" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    // A rejected key stays in the field for correction.
    expect(screen.getByLabelText("TypeSafe API 키")).toHaveValue("bad-key");
  });

  it("turns off immediately keeping the key, and clears the key on request", async () => {
    let state = { enabled: true, hasKey: true };
    const invoke = vi.fn<InvokeFn>(async (command, args) => {
      if (command === "jev_settings_save") {
        state = { enabled: Boolean(args?.enabled), hasKey: true };
      } else if (command === "jev_key_clear") {
        state = { enabled: false, hasKey: false };
      }
      return state;
    });
    renderDialog({ jev: jevActions(invoke) });
    const toggle = await screen.findByRole("switch", {
      name: "Jev 문서 검색 재정렬",
    });
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByLabelText("TypeSafe API 키")).toHaveAttribute(
      "placeholder",
      "저장된 키 사용 중 · 교체하려면 입력",
    );

    await userEvent.click(toggle);
    expect(invoke).toHaveBeenCalledWith("jev_settings_save", {
      enabled: false,
      apiKey: null,
    });
    await waitFor(() => expect(toggle).not.toBeChecked());
    expect(screen.queryByLabelText("TypeSafe API 키")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "저장된 키 삭제" }));
    expect(invoke).toHaveBeenCalledWith("jev_key_clear");
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "저장된 키 삭제" }),
      ).not.toBeInTheDocument(),
    );
  });

  it("persists at once when turned on with a stored key", async () => {
    const invoke = vi.fn<InvokeFn>(async (command, args) => {
      if (command === "jev_settings") return { enabled: false, hasKey: true };
      return { enabled: Boolean(args?.enabled), hasKey: true };
    });
    renderDialog({ jev: jevActions(invoke) });
    const toggle = await screen.findByRole("switch", {
      name: "Jev 문서 검색 재정렬",
    });
    await waitFor(() => expect(toggle).toBeEnabled());

    await userEvent.click(toggle);
    expect(invoke).toHaveBeenCalledWith("jev_settings_save", {
      enabled: true,
      apiKey: null,
    });
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByLabelText("TypeSafe API 키")).toHaveAttribute(
      "placeholder",
      "저장된 키 사용 중 · 교체하려면 입력",
    );
  });

  it("shows the backend state again when a toggle save fails", async () => {
    const invoke = vi.fn<InvokeFn>(async (command) => {
      if (command === "jev_settings") return { enabled: true, hasKey: true };
      throw "jev_key_store";
    });
    renderDialog({ jev: jevActions(invoke) });
    const toggle = await screen.findByRole("switch", {
      name: "Jev 문서 검색 재정렬",
    });
    await waitFor(() => expect(toggle).toBeChecked());

    await userEvent.click(toggle);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "OS 자격 증명 저장소에 접근하지 못했습니다.",
    );
    // The backend is still on, so the switch is too.
    expect(toggle).toBeChecked();
    expect(screen.getByLabelText("TypeSafe API 키")).toBeInTheDocument();
  });
});
