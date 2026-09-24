import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
  resolveLongChat: undefined as (() => void) | undefined,
  pendingAsk: undefined as Record<string, unknown> | undefined,
  repoState: {
    available: true,
    tracked: true,
    nested: false,
    origin: "app",
    consent: "granted",
    warning: null,
  } as Record<string, unknown>,
  commits: [] as Array<Record<string, unknown>>,
  commitDetail: undefined as Record<string, unknown> | undefined,
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    tauri.listeners.set(name, handler);
    return () => tauri.listeners.delete(name);
  }),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(async () => undefined) }));
// The EPS viewer lazy-loads Monaco; the textarea double stands in for it.
vi.mock("@/components/MonacoEditor", async () => {
  const React = await import("react");
  function MonacoEditor({
    value = "",
    language,
    readOnly,
    ariaLabel,
  }: {
    value?: string;
    language?: string;
    readOnly?: boolean;
    ariaLabel?: string;
  }) {
    return React.createElement("textarea", {
      "aria-label": ariaLabel,
      "data-language": language,
      readOnly,
      value,
      onChange: () => {},
    });
  }
  return { default: MonacoEditor, MonacoEditor };
});

import App from "./App";
const providerStatuses = [
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
    availability: "unavailable",
    selectedAsDefault: false,
    canInstall: true,
    canImport: false,
    experimental: false,
  },
  {
    provider: "antigravity",
    availability: "unavailable",
    selectedAsDefault: false,
    canInstall: false,
    canImport: false,
    experimental: true,
  },
  {
    provider: "opencode-go",
    availability: "unavailable",
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
  },
];

const providerModel = {
  provider: "codex",
  model: "gpt-test",
  displayName: "GPT Test",
  description: "test",
  isDefault: true,
  capabilities: {
    vision: true,
    toolCalls: true,
    strictStructuredOutput: true,
    reasoningLevels: ["medium"],
    nativeCompaction: true,
    hostedWebSearch: true,
  },
};


const sessionRecords = [
  {
    id: "session-a",
    name: "Session A",
    project: "Project",
    kind: "eps",
    provider: "codex",
    model: "gpt-test",
    createdAt: 1,
    lastConversationAt: 2_000,
    providerBinding: {
      provider: "codex",
      model: "gpt-test",
      reasoning: { level: "medium" },
      conversation: { provider: "codex" },
    },
    pendingRequestIds: [],
    panelLog: {
      schemaVersion: 2,
      logSeq: 1,
      log: [{
        id: 1,
        kind: "you",
        text: "previous conversation",
        clientTurnId: "77777777-7777-4777-8777-777777777777",
      }],
    },
  },
  {
    id: "session-b",
    name: "Session B",
    project: "Project",
    kind: "eps",
    provider: "codex",
    model: "gpt-test",
    createdAt: 1,
    lastConversationAt: 1_000,
    providerBinding: {
      provider: "codex",
      model: "gpt-test",
      reasoning: { level: "medium" },
      conversation: { provider: "codex" },
    },
    pendingRequestIds: [],
    panelLog: { schemaVersion: 2, logSeq: 0, log: [] },
  },
];

function emit(name: string, payload: unknown): void {
  const listener = tauri.listeners.get(name);
  if (!listener) throw new Error(`listener ${name} is not registered`);
  listener({ payload });
}

function sessionOrder(): string[] {
  const navigation = screen.getByRole("navigation", {
    name: "현재 프로젝트 세션",
  });
  return Array.from(
    navigation.querySelectorAll<HTMLButtonElement>("li > button"),
    (button) => button.getAttribute("aria-label") ?? "",
  );
}

beforeEach(() => {
  localStorage.clear();
  tauri.listeners.clear();
  tauri.resolveLongChat = undefined;
  tauri.pendingAsk = undefined;
  tauri.repoState = {
    available: true,
    tracked: true,
    nested: false,
    origin: "app",
    consent: "granted",
    warning: null,
  };
  tauri.commits = [
    { sha: "a".repeat(40), subject: "마린 체력 조정", timestamp: 1_700_000_000 },
  ];
  tauri.commitDetail = {
    sha: "a".repeat(40),
    subject: "마린 체력 조정",
    body: "session: session-a\nrequest: req-1",
    timestamp: 1_700_000_000,
    files: [
      {
        path: "src/main.eps",
        insertions: 1,
        deletions: 1,
        binary: false,
        patch: ["@@ -1 +1 @@", "-old", "+new"].join("\n"),
      },
    ],
  };
  tauri.invoke.mockReset();
  let launchPending = true;
  tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "setup_status":
        return {
          projectPath: "C:/Project",
          projectValid: true,
          euddraftPath: "C:/euddraft/euddraft.exe",
          euddraftValid: true,
          assetsReady: true,
          defaultProvider: "codex",
          providers: providerStatuses,
          projectOpened: false,
          setupRequired: false,
        };
      case "project_take_launch_request": {
        if (!launchPending) return null;
        launchPending = false;
        return { path: "C:/Project" };
      }
      case "project_open":
        return {
          projectPath: "C:/Project",
          projectValid: true,
          projectOpened: true,
          euddraftPath: "C:/euddraft/euddraft.exe",
          euddraftValid: true,
          assetsReady: true,
          defaultProvider: "codex",
          providers: providerStatuses,
          setupRequired: false,
        };
      case "status":
        return { compiling: false, project: "Project" };
      case "list":
        return { files: [] };
      case "session_list":
        return sessionRecords.map(
          ({
            id,
            name,
            project,
            kind,
            provider,
            model,
            createdAt,
            lastConversationAt,
          }) => ({
            id,
            name,
            project,
            kind,
            provider,
            model,
            createdAt,
            lastConversationAt,
          }),
        );
      case "provider_status_list":
        return providerStatuses;
      case "provider_settings": {
        const provider = String(args?.provider);
        const status =
          providerStatuses.find((candidate) => candidate.provider === provider) ??
          providerStatuses[0];
        if (provider === "ollama") {
          return {
            provider,
            status,
            models: [],
            selectedModel: null,
            selectedReasoning: null,
            baseUrl: "http://localhost:11434/v1",
            hasApiKey: false,
          };
        }
        return {
          provider: "codex",
          status,
          models: [providerModel],
          selectedModel: "gpt-test",
          selectedReasoning: { level: "medium" },
          hasApiKey: false,
        };
      }
      case "session_model_settings":
      case "session_model_settings_save":
        return {
          provider: "codex",
          models: [providerModel],
          selectedModel: "gpt-test",
          selectedReasoning: { level: "medium" },
        };
      case "session_load":
        return sessionRecords.find((record) => record.id === args?.id);
      case "session_update_log":
        return undefined;
      case "harness_jobs":
        return [];
      case "git_state":
        return tauri.repoState;
      case "git_consent_set":
        tauri.repoState = {
          ...tauri.repoState,
          consent: args?.granted === true ? "granted" : "declined",
        };
        return tauri.repoState;
      case "git_log":
        return tauri.commits;
      case "git_commit_detail":
        return tauri.commitDetail;
      case "git_revert":
        return { sha: "c".repeat(40), subject: "Revert", files: 1 };
      case "app_settings":
        return {
          notifications: {
            planApproval: { sound: true, osNotification: true },
            reviewRequired: { sound: true, osNotification: true },
            agentTurnComplete: { sound: true, osNotification: true },
            askResponseRequired: { sound: true, osNotification: true },
          },
          codexLargeContextModels: [],
          deepPlanning: false,
        };
      case "app_settings_save":
        return args?.settings;
      case "euddraft_settings":
        return {
          path: "C:/euddraft/euddraft.exe",
          valid: true,
          managed: true,
          installedVersion: "v0.10.2.5",
          updateAvailable: false,
        };
      case "euddraft_check_update":
        return {
          path: "C:/euddraft/euddraft.exe",
          valid: true,
          managed: true,
          installedVersion: "v0.10.2.5",
          latestVersion: "v0.11.0.1",
          updateAvailable: true,
        };
      case "euddraft_update":
        emit("progress", {
          stage: "euddraft_update",
          pct: 50,
          detail: "downloading euddraft v0.11.0.1",
        });
        return {
          path: "C:/euddraft/updated/euddraft.exe",
          valid: true,
          managed: true,
          installedVersion: "v0.11.0.1",
          latestVersion: "v0.11.0.1",
          updateAvailable: false,
        };
      case "attention_notify":
      case "notification_sound_preview":
        return undefined;
      case "workspace_list":
        // The tree is the whole project root, not just the agent documents.
        return {
          project: "Project",
          workspaceId: "a".repeat(64),
          files: [
            { path: "project.eap", size: 120 },
            { path: "src/main.eps", size: 40 },
            { path: "maps/source.scx", size: 2048 },
            { path: ".eud-agent/workspace/specs/index.md", size: 24 },
            { path: ".eud-agent/workspace/specs/combat.md", size: 32 },
          ],
        };
      case "workspace_read":
        if (typeof args?.path === "string" && args.path.endsWith(".scx")) {
          return {
            workspaceId: args?.workspaceId,
            path: args?.path,
            size: 2048,
            content: null,
            unreadable: "binary",
          };
        }
        return {
          workspaceId: args?.workspaceId,
          path: args?.path,
          size: 24,
          content: "# 문서 제목\n\n문서 본문",
        };
      case "workspace_search":
        return {
          workspaceId: args?.workspaceId,
          query: args?.query,
          paths: [],
        };
      case "ask_pending":
        return args?.sessionId === "session-a" ? (tauri.pendingAsk ?? null) : null;
      case "chat":
        if (args?.sessionId === "session-a") {
          return new Promise<void>((resolve) => {
            tauri.resolveLongChat = resolve;
          });
        }
        return undefined;
      case "mention_search":
        return {
          schema: "eud-mention-search/1",
          results: [
            {
              resourceKey: "map.region:region-a",
              kind: "map.region",
              label: "영역 A",
              detail: "저장된 영역 · 사각형",
              mention: {
                kind: "map.region",
                version: 1,
                projectId: "project-a",
                sourceFileSha256: "a".repeat(64),
                mapWidth: 64,
                mapHeight: 64,
                selectionId: "region-a",
                selectionSnapshotHash: "b".repeat(64),
              },
            },
          ],
          truncated: false,
        };
      default:
        return undefined;
    }
  });
});

describe("App project launcher", () => {
  it("keeps the previous project closed until an explicit open succeeds", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    const projectPath = String.raw`\\?\E:\proj\eud\proj1\native`;
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "project_take_launch_request") return null;
      if (command === "project_recent_list") {
        return [{ name: "Project", path: projectPath, lastOpenedAt: 1, available: true }];
      }
      if (command === "setup_pick_project_path") return baseInvoke?.("setup_status");
      return baseInvoke?.(command, args);
    });
    render(<App />);
    const open = await screen.findByRole("button", { name: /프로젝트 파일 열기/ });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);
    await waitFor(() => expect(open).toBeEnabled());
    expect(screen.getByRole("heading", { name: "프로젝트 시작" })).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("status");
    expect(tauri.invoke).not.toHaveBeenCalledWith("session_list");
    fireEvent.click(screen.getByRole("button", { name: String.raw`Project E:\proj\eud\proj1\native` }));
    expect(await screen.findByRole("button", { name: "Session A, 유휴" })).toBeInTheDocument();
    expect(tauri.invoke).toHaveBeenCalledWith("project_open", { path: projectPath });
  });

  it("retains a file-open request during a build for explicit retry", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    let forwarded: string | null = null;
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "project_take_launch_request" && forwarded) {
        const path = forwarded;
        forwarded = null;
        return { path };
      }
      return baseInvoke?.(command, args);
    });
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    act(() => emit("status", { compiling: true, project: "Project" }));
    forwarded = "C:/Another/project.eap";
    act(() => emit("project-open-requested", null));
    expect(await screen.findByText("C:/Another/project.eap")).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith("project_open", { path: "C:/Another/project.eap" });
    act(() => emit("status", { compiling: false, project: "Project" }));
    fireEvent.click(screen.getByRole("button", { name: "다시 열기" }));
    await screen.findByRole("button", { name: "Session A, 유휴" });
    expect(screen.queryByText("대기 중인 프로젝트 열기")).not.toBeInTheDocument();
    expect(tauri.invoke).toHaveBeenCalledWith("project_open", { path: "C:/Another/project.eap" });
  });
});

afterEach(() => {
  tauri.resolveLongChat?.();
  vi.clearAllMocks();
});

describe("App concurrent sessions", () => {
  it("invokes session B before session A's unresolved chat completes", async () => {
    render(<App />);
    await waitFor(() =>
      expect(tauri.invoke.mock.calls.map(([command]) => command)).toContain(
        "session_list",
      ),
    );
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("session_load", {
        id: "session-a",
      }),
    );
    await waitFor(() =>
      expect(
        screen.queryByText(/세션을 불러오지 못했습니다/),
      ).not.toBeInTheDocument(),
    );
    await screen.findByRole("button", { name: "Session A, 유휴" });
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "long analysis" } });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-a",
        clientTurnId: expect.any(String),
        text: "long analysis",
        attachments: [],
        mentions: [],
        executionMode: "interactive",
      });
    });
    expect(tauri.resolveLongChat).toBeTypeOf("function");

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    const secondInput = screen.getByRole("combobox", { name: "지시 입력" });
    fireEvent.change(secondInput, { target: { value: "short answer" } });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-b",
        clientTurnId: expect.any(String),
        text: "short answer",
        attachments: [],
        mentions: [],
        executionMode: "interactive",
      });
    });
    expect(tauri.resolveLongChat).toBeTypeOf("function");
  });

  it("moves the session with a newly sent chat to the top", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    expect(sessionOrder()).toEqual(["Session A, 유휴", "Session B, 유휴"]);

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    fireEvent.change(screen.getByRole("combobox", { name: "지시 입력" }), {
      target: { value: "newest conversation" },
    });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("chat", {
        sessionId: "session-b",
        clientTurnId: expect.any(String),
        text: "newest conversation",
        attachments: [],
        mentions: [],
        executionMode: "interactive",
      });
    });
    expect(sessionOrder()).toEqual(["Session B, 유휴", "Session A, 유휴"]);
  });

  it("mints a new branch turn id after edit and rewind", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "메시지 수정" }));
    await waitFor(() => expect(input).toHaveValue("previous conversation"));
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => {
      const chat = tauri.invoke.mock.calls.find(
        ([command]) => command === "chat",
      );
      const args = chat?.[1];
      expect(args).toBeDefined();
      expect(args).toHaveProperty("clientTurnId");
      expect(args).not.toHaveProperty(
        "clientTurnId",
        "77777777-7777-4777-8777-777777777777",
      );
    });
  });

  it("restores text and mention chips after backend validation rejection", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "chat" && Array.isArray(args?.mentions) && args.mentions.length > 0) {
        throw new Error("저장 영역이 변경되었습니다");
      }
      if (command === "mention_search") {
        return {
          schema: "eud-mention-search/1",
          results: [
            {
              resourceKey: "map.region:region-a",
              kind: "map.region",
              label: "영역 A",
              mention: {
                kind: "map.region",
                version: 1,
                projectId: "project-a",
                sourceFileSha256: "a".repeat(64),
                mapWidth: 64,
                mapHeight: 64,
                selectionId: "region-a",
                selectionSnapshotHash: "b".repeat(64),
              },
            },
          ],
          truncated: false,
        };
      }
      if (command === "status") return { compiling: false, project: "Project" };
      if (command === "list") return { files: [] };
      if (command === "session_list") {
        return sessionRecords.map(
          ({
            id,
            name,
            project,
            kind,
            provider,
            model,
            createdAt,
            lastConversationAt,
          }) => ({
            id,
            name,
            project,
            kind,
            provider,
            model,
            createdAt,
            lastConversationAt,
          }),
        );
      }
      if (command === "session_load") {
        return sessionRecords.find((record) => record.id === args?.id);
      }
      if (command === "harness_jobs") return [];
      if (command === "ask_pending") return null;
      if (command === "setup_status") {
        return {
          projectPath: "C:/Project",
          projectValid: true,
          euddraftPath: "C:/euddraft/euddraft.exe",
          euddraftValid: true,
          assetsReady: true,
          defaultProvider: "codex",
          providers: providerStatuses,
          projectOpened: false,
          setupRequired: false,
        };
      }
      return baseInvoke?.(command, args);
    });
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.change(input, { target: { value: "처리해 줘 @영역 A" } });
    await fireEvent.click(await screen.findByRole("option", { name: /@영역 A/ }));
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(input).toHaveValue("처리해 줘"));
    expect(screen.getAllByTestId("mention-chips")).toHaveLength(2);
    expect(screen.getAllByTestId("mention-chips").at(-1)).toHaveTextContent("@영역 A");
    await waitFor(() => {
      const autosave = tauri.invoke.mock.calls.find(
        ([command]) => command === "session_update_log",
      );
      expect(
        (
          autosave?.[1] as
            | { panelLog?: { log?: Array<{ mentions?: unknown[] }> } }
            | undefined
        )?.panelLog?.log?.some((entry) => entry.mentions?.length === 1),
      ).toBe(true);
    });
    const clientTurnIdOf = (value: unknown): string | undefined => {
      if (
        value !== null &&
        typeof value === "object" &&
        "clientTurnId" in value &&
        typeof value.clientTurnId === "string"
      ) {
        return value.clientTurnId;
      }
      return undefined;
    };
    const firstChat = tauri.invoke.mock.calls.find(
      ([command]) => command === "chat",
    )?.[1];
    await waitFor(() => expect(input).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => {
      const chats = tauri.invoke.mock.calls.filter(
        ([command]) => command === "chat",
      );
      expect(chats).toHaveLength(2);
      expect(clientTurnIdOf(chats[1][1])).toBe(clientTurnIdOf(firstChat));
    });
  });
  it("forwards mentions through the plan-feedback channel", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await waitFor(() => expect(tauri.listeners.has("plan")).toBe(true));
    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획",
        revision: 1,
      });
    });

    fireEvent.change(input, { target: { value: "@영역 A" } });
    fireEvent.click(await screen.findByRole("option", { name: /@영역 A/ }));
    fireEvent.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith(
        "plan_feedback",
        expect.objectContaining({
          sessionId: "session-a",
          text: "",
          attachments: [],
          mentions: [
            expect.objectContaining({
              label: "영역 A",
              mention: expect.objectContaining({ kind: "map.region" }),
            }),
          ],
        }),
      );
    });
  });


  it("records the engine-started continuation turn after a team map task settled", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());
    await waitFor(() => expect(tauri.listeners.has("team_task")).toBe(true));
    const task = {
      id: "task-1",
      parentRequestId: "req-1",
      mapSessionId: "map-1",
      goal: "사냥터를 꾸며줘",
      layers: ["terrain", "doodads"],
      sourceMapSha256AtCreate: "a".repeat(64),
      status: { kind: "candidate_ready" },
      candidate: {
        revision: 1,
        revisionKey: "r1:abc",
        mapSha256: "b".repeat(64),
        summary: "두다드 44건",
        terrainCells: 0,
        units: 0,
        buildings: 0,
        doodads: 44,
        sprites: 0,
        locations: 0,
      },
      createdAt: 1,
      updatedAt: 2,
    };
    act(() => {
      emit("team_task", {
        sessionId: "session-a",
        task,
        continuation: {
          clientTurnId: "turn-continue",
          text: "맵 작업 결과를 확인했습니다. [map tasks] 상태를 기준으로 이어서 진행해 주세요.",
        },
      });
    });

    // The settlement notice and the turn's own user bubble both land, and the
    // turn is in flight (send-gated) without the panel having sent anything.
    expect(
      await screen.findByText(/맵 에이전트가 후보 r1을 만들었습니다\. AI가 이어서 검토합니다/),
    ).toBeInTheDocument();
    expect(
      screen.getByText("맵 작업 결과를 확인했습니다. [map tasks] 상태를 기준으로 이어서 진행해 주세요."),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "실행" })).toBeDisabled();
    expect(screen.getByTestId("active-turn-status")).toBeInTheDocument();
    expect(tauri.invoke.mock.calls.filter(([command]) => command === "chat")).toHaveLength(0);
  });

  it("does not autosave conversation logs for project-state refreshes", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("status")).toBe(true));
    tauri.invoke.mockClear();

    act(() => {
      emit("status", { compiling: false, project: "Project" });
    });
    const delay = Promise.withResolvers<void>();
    window.setTimeout(delay.resolve, 550);
    await act(async () => {
      await delay.promise;
    });

    expect(
      tauri.invoke.mock.calls.filter(
        ([command]) => command === "session_update_log",
      ),
    ).toHaveLength(0);
  });

  it("never persists transient provider progress rows", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("progress")).toBe(true));
    tauri.invoke.mockClear();

    act(() => {
      emit("progress", {
        sessionId: "session-a",
        stage: "provider",
        detail: "Ollama turn started",
      });
    });
    const delay = Promise.withResolvers<void>();
    window.setTimeout(delay.resolve, 550);
    await act(async () => {
      await delay.promise;
    });

    const autosave = tauri.invoke.mock.calls.find(
      ([command]) => command === "session_update_log",
    );
    expect(autosave).toBeDefined();
    const autosaveArgs = autosave?.[1];
    if (
      !autosaveArgs ||
      typeof autosaveArgs !== "object" ||
      !("panelLog" in autosaveArgs) ||
      !autosaveArgs.panelLog ||
      typeof autosaveArgs.panelLog !== "object" ||
      !("log" in autosaveArgs.panelLog) ||
      !Array.isArray(autosaveArgs.panelLog.log)
    ) {
      throw new Error("session_update_log payload is invalid");
    }
    expect(autosaveArgs.panelLog.log).not.toContainEqual(
      expect.objectContaining({ kind: "progress" }),
    );
  });

  it("routes interleaved events and backend activities only to addressed sessions", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("session_activity")).toBe(true));

    act(() => {
      emit("answer", { sessionId: "session-b", text: "B-only answer" });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_read",
      });
      emit("session_activity", {
        sessionId: "session-b",
        activity: "running_write",
      });
    });

    expect(screen.getByRole("button", { name: "Session A, 분석 중" })).toBeInTheDocument();
    const writingRow = screen.getByRole("button", {
      name: "Session B, 변경 중",
    });
    expect(writingRow).toBeInTheDocument();
    expect(screen.queryByText("B-only answer")).not.toBeInTheDocument();

    fireEvent.click(writingRow);
    expect(await screen.findByText("B-only answer")).toBeInTheDocument();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "session_open"),
    ).toHaveLength(0);
  });
  it("routes autonomous lifecycle independently from session activity", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("autonomous_run")).toBe(true));

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_read",
      });
      emit("autonomous_run", {
        sessionId: "session-a",
        schemaVersion: 1,
        id: "auto-a",
        status: "paused_after_restart",
        startedAt: 1,
        updatedAt: 2,
        iteration: 4,
        goal: "긴 작업",
        requestId: "request-a",
        projectId: "ExampleProject",
        clientTurnId: "11111111-1111-4111-8111-111111111111",
        projectRevision: "revision-a",
        policy: {
          maxWallTimeMillis: 14_400_000,
        },
        progress: {
          elapsedActiveMillis: 3_000,
          readActions: 12,
          writeActions: 5,
          consecutiveNoProgress: 0,
          recentFingerprints: [],
        },
        pauseReason: "restart",
        blocker: "앱 재시작 후 명시적으로 계속해야 합니다.",
      });
    });

    expect(screen.getByText("앱 재시작 후 일시 중지")).toBeInTheDocument();
    expect(screen.getByText("반복 4")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Session A, 분석 중" }),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    expect(screen.queryByText("앱 재시작 후 일시 중지")).not.toBeInTheDocument();
  });


  it("routes post-acceptance harness actions and closes completed jobs automatically", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("harness_job")).toBe(true));

    act(() => {
      emit("harness_job", {
        sessionId: "session-a",
        requestId: "req-code",
        id: "harness-1",
        sourceRequestId: "req-code",
        status: "waiting_runtime",
        runtimeVerification: "waiting",
        attempts: 0,
        createdAt: 1,
        updatedAt: 1,
        memoryFiles: [],
        dismissed: false,
      });
    });

    expect(screen.getByText("인게임 검증 대기")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "인게임 검증 완료" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("harness_runtime_confirm", {
        jobId: "harness-1",
      }),
    );
    fireEvent.click(screen.getByRole("button", { name: "건너뛰기" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("harness_skip", {
        jobId: "harness-1",
      }),
    );
    act(() => {
      emit("harness_job", {
        sessionId: "session-a",
        id: "harness-1",
        sourceRequestId: "req-code",
        status: "completed",
        runtimeVerification: "confirmed",
        attempts: 1,
        createdAt: 1,
        updatedAt: 2,
        memoryFiles: [],
        dismissed: false,
      });
    });
    expect(screen.queryByText("하네스 완료")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "하네스 상태 닫기" })).not.toBeInTheDocument();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "harness_dismiss"),
    ).toHaveLength(0);
  });

  it("keeps context usage isolated to its addressed session", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("context_usage")).toBe(true));

    act(() => {
      emit("context_usage", {
        sessionId: "session-b",
        turnId: "turn-b",
        tokenUsage: {
          last: {
            inputTokens: 31_000,
            cachedInputTokens: 24_000,
            cacheWriteInputTokens: 0,
            outputTokens: 1_200,
            reasoningOutputTokens: 800,
            totalTokens: 32_200,
          },
          total: {
            inputTokens: 52_000,
            cachedInputTokens: 40_000,
            cacheWriteInputTokens: 600,
            outputTokens: 2_100,
            reasoningOutputTokens: 1_300,
            totalTokens: 54_100,
          },
          modelContextWindow: 128_000,
        },
      });
    });

    expect(
      screen.queryByRole("button", { name: /컨텍스트 .* 사용/ }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));

    expect(
      screen.getByRole("button", { name: /컨텍스트 .* 사용/ }),
    ).toBeInTheDocument();
  });

  it("submits ASK answers to the blocked session without opening a new turn", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "설계를 진행해 줘" } });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(tauri.resolveLongChat).toBeTypeOf("function"));
    await waitFor(() => expect(tauri.listeners.has("ask")).toBe(true));

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "waiting_input",
      });
      emit("ask", {
        sessionId: "session-a",
        requestId: "ask-1",
        questions: [
          {
            id: "mode",
            question: "방식을 고르세요.",
            multi: false,
            options: [{ label: "빠르게" }, { label: "세밀하게" }],
          },
        ],
      });
    });

    expect(
      screen.getByRole("button", { name: "Session A, 응답 필요" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("빠르게"));
    fireEvent.click(screen.getByRole("button", { name: "답변 전달" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("ask_response", {
        sessionId: "session-a",
        requestId: "ask-1",
        answers: { mode: { answers: ["빠르게"] } },
      });
    });
    expect(screen.queryByRole("region", { name: "AI 질문" })).not.toBeInTheDocument();
  });

  it("closes the ASK card when the backend reports the wait expired", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "설계를 진행해 줘" } });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));
    await waitFor(() => expect(tauri.resolveLongChat).toBeTypeOf("function"));
    await waitFor(() => expect(tauri.listeners.has("ask")).toBe(true));
    const questions = [
      {
        id: "mode",
        question: "방식을 고르세요.",
        multi: false,
        options: [{ label: "빠르게" }, { label: "세밀하게" }],
      },
    ];

    act(() => {
      emit("ask", {
        sessionId: "session-a",
        requestId: "ask-1",
        status: "pending",
        waitSeconds: 240,
        questions,
      });
    });
    expect(screen.getByRole("region", { name: "AI 질문" })).toHaveTextContent(
      "남은 시간",
    );

    act(() => {
      emit("ask", {
        sessionId: "session-a",
        requestId: "ask-1",
        status: "expired",
        waitSeconds: 240,
        questions,
      });
    });

    expect(screen.queryByRole("region", { name: "AI 질문" })).not.toBeInTheDocument();
    expect(screen.getByText(/240초/)).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith(
      "ask_response",
      expect.anything(),
    );
  });

  it("restores a pending ASK that was emitted before the panel could display it", async () => {
    tauri.pendingAsk = {
      sessionId: "session-a",
      requestId: "ask-recovered",
      questions: [
        {
          id: "mode",
          question: "복구된 질문입니다.",
          multi: false,
          options: [{ label: "계속" }, { label: "중단" }],
        },
      ],
    };

    render(<App />);

    expect(
      await screen.findByRole("button", { name: "Session A, 응답 필요" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("region", { name: "AI 질문" }),
    ).toHaveTextContent("복구된 질문입니다.");
  });

  it("opens the plan as a center tab with the prompt underneath", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => {
      expect(tauri.listeners.has("plan")).toBe(true);
      expect(tauri.listeners.has("git")).toBe(true);
    });
    act(() => {
      emit("plan", { sessionId: "session-a", markdown: "# 계획\n\n계획 본문", revision: 1 });
    });
    // The plan is a virtual tab (store markdown, no workspace read) that
    // activates on arrival; the approve control lives in the tab.
    const planTab = await screen.findByRole("tab", { name: "계획 (rev 1) 문서 탭" });
    expect(planTab).toHaveAttribute("aria-selected", "true");
    const planPanel = screen.getByRole("region", { name: "계획 검토" });
    expect(within(planPanel).getByText("계획 본문")).toBeInTheDocument();
    expect(within(planPanel).getByRole("button", { name: "승인" })).toBeEnabled();
    // The conversation keeps a one-line notice; the prompt is shared under
    // every tab so feedback is typed without leaving the plan.
    expect(screen.getByTestId("plan-review-notice")).toHaveTextContent("계획안 (rev 1)");
    expect(screen.getByRole("button", { name: "실행" })).toBeInTheDocument();

    fireEvent.click(within(planPanel).getByRole("button", { name: "승인" }));
    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("plan_approve", {
        sessionId: "session-a",
      });
    });
    // Approval keeps the tab as a read-only reference.
    expect(screen.getByRole("tab", { name: "계획 (rev 1) 문서 탭" })).toBeInTheDocument();
    expect(within(planPanel).queryByRole("button", { name: "승인" })).toBeNull();
    expect(within(planPanel).getByText("읽기 전용")).toBeInTheDocument();
    expect(screen.queryByTestId("plan-review-notice")).toBeNull();

    act(() => {
      emit("git", {
        sessionId: "session-a",
        turn: { sha: "b".repeat(40), subject: "계획대로 적용", files: 1 },
      });
    });
    // The turn's commit is a quiet conversation row, not a review surface.
    expect(await screen.findByText(/계획대로 적용/)).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "계획 (rev 1) 문서 탭" })).toBeInTheDocument();
  });

  it("keeps a closed plan tab closed across session switches and reopens it for a new revision", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("plan")).toBe(true));

    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획\n\n세션 전환 뒤에도 닫힌 상태여야 합니다.",
        revision: 1,
      });
    });
    const planTab = await screen.findByRole("tab", { name: "계획 (rev 1) 문서 탭" });
    expect(planTab).toHaveAttribute("aria-selected", "true");
    expect(
      screen.getByText("세션 전환 뒤에도 닫힌 상태여야 합니다."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "계획 (rev 1) 탭 닫기" }));
    expect(screen.queryByRole("tab", { name: "계획 (rev 1) 문서 탭" })).toBeNull();
    expect(screen.getByRole("tab", { name: "대화" })).toHaveAttribute("aria-selected", "true");
    expect(
      screen.queryByText("세션 전환 뒤에도 닫힌 상태여야 합니다."),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Session B, 유휴" }));
    fireEvent.click(screen.getByRole("button", { name: "Session A, 유휴" }));
    expect(screen.queryByRole("tab", { name: "계획 (rev 1) 문서 탭" })).toBeNull();

    // The conversation notice reopens the closed tab on demand.
    fireEvent.click(screen.getByRole("button", { name: "계획 보기" }));
    expect(screen.getByRole("tab", { name: "계획 (rev 1) 문서 탭" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    fireEvent.click(screen.getByRole("button", { name: "계획 (rev 1) 탭 닫기" }));

    // A new revision reopens and activates the tab.
    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획\n\n수정된 계획입니다.",
        revision: 2,
      });
    });
    const revised = await screen.findByRole("tab", { name: "계획 (rev 2) 문서 탭" });
    expect(revised).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("수정된 계획입니다.")).toBeInTheDocument();
  });

  it("keeps another session writable while the first session is in review", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("session_activity")).toBe(true));

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "review",
      });
      emit("session_activity", {
        sessionId: "session-b",
        activity: "running_write",
      });
    });

    expect(
      screen.getByRole("button", { name: "Session A, 검토 필요" }),
    ).toBeInTheDocument();
    const writingRow = screen.getByRole("button", {
      name: "Session B, 변경 중",
    });
    expect(writingRow).toBeInTheDocument();

    fireEvent.click(writingRow);
    expect(
      await screen.findByText("격리 워크스페이스에서 변경을 시작합니다."),
    ).toBeInTheDocument();
  });

  it("announces a commit of edits made outside the app", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("git")).toBe(true));

    act(() => {
      emit("git", {
        sessionId: "session-a",
        external: { sha: "a".repeat(40), subject: "앱 밖 편집", files: 3 },
        turn: { sha: "b".repeat(40), subject: "마린 체력 조정", files: 1 },
      });
    });

    // A commit the user did not ask for gets a sentence naming the file count.
    expect(
      await screen.findByText(/앱 밖에서 바뀐 파일 3개/),
    ).toBeInTheDocument();
  });

  it("reports a failed record without claiming the turn failed", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("git")).toBe(true));

    act(() => {
      emit("git", {
        sessionId: "session-a",
        warning: "이 턴을 기록하지 못했습니다: 저장소가 잠겼습니다.",
      });
    });

    expect(
      await screen.findByText("이 턴을 기록하지 못했습니다: 저장소가 잠겼습니다."),
    ).toBeInTheDocument();
  });
});

describe("App project history", () => {
  it("lists the project's commits where request review used to sit", async () => {
    render(<App />);
    const history = await screen.findByRole("region", { name: "변경 기록" });
    expect(tauri.invoke).toHaveBeenCalledWith("git_state");
    // The record is collapsed by default: it is history, not a demand for a
    // decision, so it never takes the conversation's room unasked.
    fireEvent.click(
      within(history).getByRole("button", { name: "변경 기록 펼치기" }),
    );
    // The row and the opened commit's heading both carry the subject.
    expect(
      await within(history).findAllByText("마린 체력 조정"),
    ).not.toHaveLength(0);
    expect(
      within(history).getByTestId("commit-file-src/main.eps"),
    ).toHaveTextContent("+new");
  });

  it("asks once before committing into a repository the user already had", async () => {
    tauri.repoState = {
      available: true,
      tracked: true,
      nested: false,
      origin: "preexisting",
      consent: "pending",
      warning: null,
    };
    render(<App />);

    const dialog = await screen.findByRole("dialog", {
      name: /git 기록에 커밋해도 될까요/,
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "커밋을 허용합니다" }),
    );

    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("git_consent_set", {
        granted: true,
      }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("App native compaction", () => {
  it("routes an exact /compact command without starting a chat turn", async () => {
    render(<App />);
    const input = await screen.findByRole("combobox", { name: "지시 입력" });
    await waitFor(() => expect(input).toBeEnabled());

    fireEvent.change(input, { target: { value: "/compact" } });
    fireEvent.click(screen.getByRole("button", { name: "실행" }));

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("compact", {
        sessionId: "session-a",
      });
    });
    expect(
      await screen.findByText("대화 컨텍스트를 압축했습니다."),
    ).toBeInTheDocument();
    expect(tauri.invoke).not.toHaveBeenCalledWith(
      "chat",
      expect.objectContaining({ text: "/compact" }),
    );
  });
});

describe("App notifications", () => {
  it("opens general settings and persists event channel toggles", async () => {
    render(<App />);
    const settingsButton = await screen.findByRole("button", {
      name: "설정 열기",
    });
    fireEvent.click(settingsButton);

    expect(await screen.findByRole("dialog", { name: "설정" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "알림" }));
    fireEvent.click(
      screen.getByRole("switch", { name: "계획 승인 필요 알림음" }),
    );

    await waitFor(() => {
      expect(tauri.invoke).toHaveBeenCalledWith("app_settings_save", {
        settings: {
          notifications: {
            planApproval: { sound: false, osNotification: true },
            reviewRequired: { sound: true, osNotification: true },
            agentTurnComplete: { sound: true, osNotification: true },
            askResponseRequired: { sound: true, osNotification: true },
          },
          codexLargeContextModels: [],
          deepPlanning: false,
          scmdraftPath: "",
        },
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "소리 미리듣기" }));
    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("notification_sound_preview"),
    );
  });

  it("checks and applies an euddraft update from compile settings", async () => {
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "설정 열기" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "컴파일" }));

    expect(
      await screen.findByText("C:/euddraft/euddraft.exe"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "최신 버전 확인" }));
    expect(
      await screen.findByText(/새 euddraft v0\.11\.0\.1 버전을 설치할 수 있습니다/),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "최신 버전으로 업데이트" }),
    );

    expect(
      await screen.findByText("C:/euddraft/updated/euddraft.exe"),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "설정" })).toBeInTheDocument();
    expect(screen.queryByText("euddraft_update")).not.toBeInTheDocument();
    expect(screen.getByText("최신 버전을 사용 중입니다.")).toBeInTheDocument();
    expect(tauri.invoke).toHaveBeenCalledWith("euddraft_check_update");
    expect(tauri.invoke).toHaveBeenCalledWith("euddraft_update");
  });

  it("opens 설정 > 컴파일 when SCMDraft 2 has no executable yet, and launches once it does", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    let launch: { kind: "launched" | "unconfigured" } = { kind: "unconfigured" };
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "project_open_scmdraft") return launch;
      return baseInvoke?.(command, args);
    });
    render(<App />);
    const open = await screen.findByRole("button", { name: "SCMDraft 2로 열기" });
    await waitFor(() => expect(open).toBeEnabled());
    fireEvent.click(open);

    const dialog = await screen.findByRole("dialog", { name: "설정" });
    expect(within(dialog).getByRole("button", { name: "컴파일" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(within(dialog).getByRole("heading", { name: "SCMDraft 2" })).toBeInTheDocument();
    expect(tauri.invoke).toHaveBeenCalledWith("project_open_scmdraft");

    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "설정" })).not.toBeInTheDocument(),
    );
    launch = { kind: "launched" };
    fireEvent.click(screen.getByRole("button", { name: "SCMDraft 2로 열기" }));
    await waitFor(() =>
      expect(tauri.invoke.mock.calls.filter(([command]) => command === "project_open_scmdraft")).toHaveLength(2),
    );
    expect(screen.queryByRole("dialog", { name: "설정" })).not.toBeInTheDocument();
  });

  it("notifies once when a new plan arrives", async () => {
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("plan")).toBe(true));

    act(() => {
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획",
        revision: 1,
      });
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 계획",
        revision: 1,
      });
      emit("plan", {
        sessionId: "session-a",
        markdown: "# 수정 계획",
        revision: 2,
      });
    });

    await waitFor(() => {
      const attentionCalls = tauri.invoke.mock.calls.filter(
        ([command]) => command === "attention_notify",
      );
      expect(attentionCalls).toEqual([
        [
          "attention_notify",
          { kind: "planApproval", showOs: true, sessionId: "session-a" },
        ],
        [
          "attention_notify",
          { kind: "planApproval", showOs: true, sessionId: "session-a" },
        ],
      ]);
    });
    focus.mockRestore();
  });

  it("notifies once for a new ASK request", async () => {
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() => expect(tauri.listeners.has("ask")).toBe(true));
    tauri.invoke.mockClear();

    const payload = {
      sessionId: "session-a",
      requestId: "ask-1",
      questions: [
        {
          id: "mode",
          question: "방식을 고르세요.",
          multi: false,
          options: [],
        },
      ],
    };
    act(() => {
      emit("ask", payload);
      emit("ask", payload);
    });

    await waitFor(() => {
      const attentionCalls = tauri.invoke.mock.calls.filter(
        ([command]) => command === "attention_notify",
      );
      expect(attentionCalls).toEqual([
        [
          "attention_notify",
          {
            kind: "askResponseRequired",
            showOs: true,
            sessionId: "session-a",
          },
        ],
      ]);
    });
    focus.mockRestore();
  });

  it("notifies when ordinary turns settle but not when review starts", async () => {
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() =>
      expect(tauri.listeners.has("session_activity")).toBe(true),
    );
    tauri.invoke.mockClear();

    act(() => {
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_read",
      });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "idle",
      });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_read",
      });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "review",
      });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "running_write",
      });
      emit("session_activity", {
        sessionId: "session-a",
        activity: "error",
      });
    });

    await waitFor(() => {
      const attentionCalls = tauri.invoke.mock.calls.filter(
        ([command]) => command === "attention_notify",
      );
      expect(attentionCalls).toEqual([
        [
          "attention_notify",
          {
            kind: "agentTurnComplete",
            showOs: true,
            sessionId: "session-a",
          },
        ],
        [
          "attention_notify",
          {
            kind: "agentTurnComplete",
            showOs: true,
            sessionId: "session-a",
          },
        ],
      ]);
    });
    focus.mockRestore();
  });

  it("selects the session named by a clicked OS notification", async () => {
    render(<App />);
    await screen.findByRole("button", { name: "Session A, 유휴" });
    await waitFor(() =>
      expect(tauri.listeners.has("notification_activated")).toBe(true),
    );

    act(() => {
      emit("notification_activated", { sessionId: "session-b" });
    });

    expect(
      screen.getByRole("button", { name: "Session B, 유휴" }),
    ).toHaveAttribute("aria-current", "page");
  });
});

describe("App setup payload compatibility", () => {
  it("continues to provider setup after an explicit open with nullable options", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "setup_status" || command === "project_open") {
        return {
          projectPath: "C:/Project",
          projectValid: true,
          euddraftPath: "C:/euddraft/euddraft.exe",
          euddraftValid: true,
          assetsReady: true,
          defaultProvider: null,
          providers: providerStatuses.map((status) => ({
            ...status,
            availability: "unavailable",
            selectedAsDefault: false,
            detailCode: null,
          })),
          projectOpened: command === "project_open",
          setupRequired: true,
          error: null,
        };
      }
      return baseInvoke?.(command, args);
    });

    render(<App />);
    expect(
      await screen.findByRole("combobox", { name: "기본 AI 제공자 선택" }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Unknown IPC message payload/)).not.toBeInTheDocument();
  });
});

describe("App provider login cancellation", () => {
  it("cancels the exact pending attempt and stops the waiting state", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    const firstRunStatuses = providerStatuses.map((status) => ({
      ...status,
      availability:
        status.provider === "antigravity"
          ? ("needs-authentication" as const)
          : status.availability,
      selectedAsDefault: status.provider === "antigravity",
    }));
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "setup_status" || command === "project_open") {
        return {
          projectPath: "C:/Project",
          projectValid: true,
          euddraftPath: "C:/euddraft/euddraft.exe",
          euddraftValid: true,
          assetsReady: true,
          defaultProvider: "antigravity",
          providers: firstRunStatuses,
          projectOpened: command === "project_open",
          setupRequired: true,
        };
      }
      if (command === "provider_login_start") return "attempt-antigravity";
      if (command === "provider_login_cancel") return undefined;
      if (command === "provider_login_status") {
        return firstRunStatuses.find((status) => status.provider === args?.provider);
      }
      return baseInvoke?.(command, args);
    });

    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "Google 로그인" }));
    fireEvent.click(await screen.findByRole("button", { name: "로그인 취소" }));

    await waitFor(() =>
      expect(tauri.invoke).toHaveBeenCalledWith("provider_login_cancel", {
        provider: "antigravity",
        attemptId: "attempt-antigravity",
      }),
    );
    expect(await screen.findByRole("button", { name: "Google 로그인" })).toBeInTheDocument();
    expect(screen.getByText("연결 작업이 취소되었습니다.")).toBeInTheDocument();
  });
});

describe("App project tools sidebar", () => {
  it("closes and reopens the tabbed sidebar with one header toggle", async () => {
    render(<App />);

    expect(
      await screen.findByRole("complementary", { name: "프로젝트 도구" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "DAT 위키" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "메모리" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "파일" })).toBeInTheDocument();
    expect(
      within(screen.getByRole("complementary", { name: "프로젝트 도구" })).queryByRole(
        "button",
        { name: /닫기/ },
      ),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "프로젝트 도구 닫기" }),
    );
    expect(
      screen.queryByRole("complementary", { name: "프로젝트 도구" }),
    ).not.toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "프로젝트 도구 열기" }),
    );
    expect(
      screen.getByRole("complementary", { name: "프로젝트 도구" }),
    ).toBeInTheDocument();
  });
});

describe("App center document tabs", () => {
  it("opens workspace files as center tabs and returns to the conversation", async () => {
    render(<App />);

    // The file tree is the default right-panel tab and loads with the project.
    const sidebar = await screen.findByRole("complementary", {
      name: "프로젝트 도구",
    });
    expect(screen.getByRole("tab", { name: "파일" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    // The root lists the real project layout, folders first.
    expect(within(sidebar).getByRole("button", { name: /project\.eap/ })).toBeInTheDocument();
    for (const folder of [".eud-agent", "workspace", "specs"]) {
      fireEvent.click(
        await within(sidebar).findByRole("button", { name: `${folder} 폴더 펼치기` }),
      );
    }
    fireEvent.click(
      await within(sidebar).findByRole("button", { name: /combat\.md/ }),
    );

    // A center tab opens with the document body; the tree keeps its place.
    const documentTab = await screen.findByRole("tab", {
      name: "combat.md 문서 탭",
    });
    expect(documentTab).toHaveAttribute("aria-selected", "true");
    await screen.findByText(".eud-agent/workspace/specs/combat.md");
    expect(screen.getByText(/문서 본문/)).toBeInTheDocument();
    expect(screen.getByText("검토 대상 문서")).toBeInTheDocument();
    expect(
      within(sidebar).getByRole("button", { name: /combat\.md/ }),
    ).toHaveAttribute("aria-current", "page");

    // An EPS source is a plain project file shown in the read-only Monaco
    // viewer with the TypeScript grammar; a map opens as a notice, not text.
    fireEvent.click(await within(sidebar).findByRole("button", { name: "src 폴더 펼치기" }));
    fireEvent.click(await within(sidebar).findByRole("button", { name: /main\.eps/ }));
    await screen.findByText("src/main.eps");
    expect(screen.getByText("프로젝트 파일")).toBeInTheDocument();
    const epsEditor = await screen.findByRole("textbox", { name: "src/main.eps 소스" });
    expect(epsEditor).toHaveAttribute("data-language", "typescript");
    expect(epsEditor).toHaveAttribute("readonly");
    fireEvent.click(await within(sidebar).findByRole("button", { name: "maps 폴더 펼치기" }));
    fireEvent.click(await within(sidebar).findByRole("button", { name: /source\.scx/ }));
    await screen.findByRole("tab", { name: "source.scx 문서 탭" });
    expect(await screen.findByRole("status")).toHaveTextContent(/바이너리 · 2 KB/);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "combat.md 문서 탭" }));

    // The conversation tab still exists and reactivates without refetching.
    fireEvent.click(screen.getByRole("tab", { name: "대화" }));
    expect(screen.getByRole("tab", { name: "대화" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(documentTab).toHaveAttribute("aria-selected", "false");

    // Closing the document tab removes it and returns to the conversation.
    fireEvent.click(
      screen.getByRole("button", { name: "combat.md 탭 닫기" }),
    );
    expect(
      screen.queryByRole("tab", { name: "combat.md 문서 탭" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "대화" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("closes the active document tab with Ctrl+W", async () => {
    render(<App />);
    const sidebar = await screen.findByRole("complementary", {
      name: "프로젝트 도구",
    });
    for (const folder of [".eud-agent", "workspace", "specs"]) {
      fireEvent.click(
        await within(sidebar).findByRole("button", { name: `${folder} 폴더 펼치기` }),
      );
    }
    fireEvent.click(
      await within(sidebar).findByRole("button", { name: "combat.md" }),
    );
    await screen.findByRole("tab", { name: "combat.md 문서 탭" });

    fireEvent.keyDown(window, { key: "w", ctrlKey: true });
    expect(
      screen.queryByRole("tab", { name: "combat.md 문서 탭" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "대화" })).toHaveAttribute(
      "aria-selected",
      "true",
    );

    // Ctrl+W never closes the pinned conversation tab.
    fireEvent.keyDown(window, { key: "w", ctrlKey: true });
    expect(screen.getByRole("tab", { name: "대화" })).toBeInTheDocument();
  });
});

describe("App document tab cap", () => {
  it("replaces the active document tab instead of growing past the cap", async () => {
    const baseInvoke = tauri.invoke.getMockImplementation();
    const files = Array.from({ length: 12 }, (_, index) => ({
      path: `specs/doc-${index}.md`,
      size: 16,
    }));
    tauri.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "workspace_list") {
        return { project: "Project", workspaceId: "a".repeat(64), files };
      }
      return baseInvoke?.(command, args);
    });

    render(<App />);
    const sidebar = await screen.findByRole("complementary", {
      name: "프로젝트 도구",
    });
    fireEvent.click(
      await within(sidebar).findByRole("button", { name: "specs 폴더 펼치기" }),
    );

    // Open more files than the cap allows.
    for (const { path } of files) {
      const leaf = path.slice(path.lastIndexOf("/") + 1);
      fireEvent.click(
        await within(sidebar).findByRole("button", {
          name: new RegExp(`^${leaf.replace(".", "\\.")}$`),
        }),
      );
    }

    const strip = screen.getByRole("tablist", { name: "열린 문서" });
    await waitFor(() => {
      // 1 pinned 대화 tab + at most 8 document tabs.
      expect(within(strip).getAllByRole("tab").length).toBeLessThanOrEqual(9);
    });
    // The newest file is active; the previously active tab was replaced, so
    // the earliest opened tabs survive (preview-reuse, not FIFO eviction).
    expect(
      screen.getByRole("tab", { name: "doc-11.md 문서 탭" }),
    ).toHaveAttribute("aria-selected", "true");
    expect(
      screen.queryByRole("tab", { name: "doc-10.md 문서 탭" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: "doc-0.md 문서 탭" }),
    ).toBeInTheDocument();
  });
});
