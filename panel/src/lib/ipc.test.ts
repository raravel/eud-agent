import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  IpcClient,
  appSettingsGet,
  appSettingsSave,
  attentionNotify,
  euddraftCheckUpdate,
  gitCommitDetail,
  gitConsentSet,
  gitLog,
  gitRevert,
  gitState,
  euddraftSettingsGet,
  euddraftUpdate,
  providerBaseUrlSave,
  providerDefaultsSave,
  providerSettingsGet,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  compactSession,
  isAgentTurnEndTransition,
  notificationSoundPreview,
  mentionSearch,
  openScmdraft,
  openProjectRootIn,
  runProjectBuild,
  pickScmdraftPath,
  projectExportE3s,
  workspaceList,
  workspaceRead,
  workspaceSearch,
} from "@/lib/ipc";
import type { ClientMessage, ServerMessage } from "@/lib/ipc";

type UnlistenFn = () => void;
type ListenHandler = (event: { payload: unknown }) => void;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function flushMicrotasks() {
  return Promise.resolve();
}

function makeHarness() {
  const listeners = new Map<string, ListenHandler>();
  const unlisteners: UnlistenFn[] = [];
  const invoke = vi.fn();
  const listen = vi.fn(async (event: string, handler: ListenHandler) => {
    listeners.set(event, handler);
    const unlisten = vi.fn();
    unlisteners.push(unlisten);
    return unlisten;
  });
  return { invoke, listen, listeners, unlisteners };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("project refresh invalidation", () => {
  it("never publishes an old source list after a same-name project switch", async () => {
    const { invoke, listen } = makeHarness();
    const oldList = deferred<unknown>();
    const listStarted = deferred<void>();
    let firstList = true;
    const newFile = { path: "src/new.eps", ftype: "CUIEps", settable: true };
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") return { project: "Same name", compiling: false };
      if (command === "list") {
        if (firstList) {
          firstList = false;
          listStarted.resolve();
          return oldList.promise;
        }
        return { files: [newFile] };
      }
    });
    const messages: ServerMessage[] = [];
    const client = new IpcClient({ invoke, listen, onMessage: (message) => messages.push(message) });
    await client.connect();
    const stale = client.refresh();
    await listStarted.promise;
    client.invalidateProject();
    await client.refresh();
    oldList.resolve({ files: [{ path: "src/old.eps", ftype: "CUIEps", settable: true }] });
    await stale;
    expect(messages.filter((message) => message.type === "list")).toEqual([
      { type: "list", files: [newFile] },
    ]);
    client.stop();
  });
});

describe("send", () => {
  it("sends chat via invoke", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
    });
    const mention = {
      id: "mention-1",
      label: "영역 A",
      mention: {
        kind: "map.region" as const,
        version: 1 as const,
        projectId: "project-a",
        sourceFileSha256: "a".repeat(64),
        mapWidth: 64,
        mapHeight: 64,
        selectionId: "region-a",
        selectionSnapshotHash: "b".repeat(64),
      },
    };
    const msg: ClientMessage = {
      type: "chat",
      sessionId: "session-a",
      clientTurnId: "11111111-1111-4111-8111-111111111111",
      text: "hello",
      attachments: ["image-1"],
      mentions: [mention],
    };

    await client.send(msg);

    expect(invoke).toHaveBeenCalledWith("chat", {
      sessionId: "session-a",
      clientTurnId: "11111111-1111-4111-8111-111111111111",
      text: "hello",
      attachments: ["image-1"],
      mentions: [mention],
      executionMode: "interactive",
    });
  });

  it("sends plan feedback with the same generic mentions field", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({ invoke, listen, onMessage: () => {} });
    const mentions = [
      {
        id: "mention-location",
        label: "회복 지점",
        mention: {
          kind: "map.location" as const,
          version: 1 as const,
          projectId: "project-a",
          sourceFileSha256: "a".repeat(64),
          locationId: 17,
          locationFingerprint: "c".repeat(64),
        },
      },
    ];

    await client.send({
      type: "plan_feedback",
      sessionId: "session-a",
      clientTurnId: "22222222-2222-4222-8222-222222222222",
      text: "반영해 줘",
      attachments: [],
      mentions,
    });

    expect(invoke).toHaveBeenCalledWith("plan_feedback", {
      sessionId: "session-a",
      clientTurnId: "22222222-2222-4222-8222-222222222222",
      text: "반영해 줘",
      attachments: [],
      mentions,
    });
  });

  it("returns ASK answers without starting a new chat turn", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
    });

    await client.send({
      type: "ask_response",
      sessionId: "session-a",
      requestId: "ask-1",
      answers: {
        mode: { answers: ["빠르게"] },
        features: { answers: ["로그", "진행률"] },
      },
    });

    expect(invoke).toHaveBeenCalledWith("ask_response", {
      sessionId: "session-a",
      requestId: "ask-1",
      answers: {
        mode: { answers: ["빠르게"] },
        features: { answers: ["로그", "진행률"] },
      },
    });
  });

  it("sends conversation_rewind with the durable log prefix", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
    });
    const panelLog = {
      schemaVersion: 2,
      logSeq: 1,
      log: [{ id: 1, kind: "you", text: "첫 요청" }],
    };

    await client.send({
      type: "conversation_rewind",
      sessionId: "session-a",
      panelLog,
    });

    expect(invoke).toHaveBeenCalledWith("conversation_rewind", {
      sessionId: "session-a",
      panelLog,
    });
  });
  it("sends reviewed harness issue ids without legacy fields", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({ invoke, listen, onMessage: () => {} });

    await client.send({
      type: "setup_import_e3s",
      sourceE3s: "C:\\Legacy\\sample.e3s",
      destination: "C:\\Work\\ImportedProject",
      excludedImportItems: ["workspace-1", "memory-1"],
    });

    expect(invoke).toHaveBeenCalledWith("setup_import_e3s", {
      request: {
        sourceE3s: "C:\\Legacy\\sample.e3s",
        destination: "C:\\Work\\ImportedProject",
        excludedImportItems: ["workspace-1", "memory-1"],
      },
    });
  });

});

describe("mention search", () => {
  it("sends one bounded request envelope and preserves opaque snapshots", async () => {
    const invoke = vi.fn().mockResolvedValue({
      schema: "eud-mention-search/1",
      results: [
        {
          resourceKey: "map.location:17",
          kind: "map.location",
          label: "회복 지점",
          mention: {
            kind: "map.location",
            version: 1,
            projectId: "project-a",
            sourceFileSha256: "a".repeat(64),
            locationId: 17,
            locationFingerprint: "c".repeat(64),
          },
        },
      ],
      truncated: false,
    });
    const request = { query: "회복 지점", kinds: ["map.location" as const], limit: 20 };

    const result = await mentionSearch(request, invoke);

    expect(invoke).toHaveBeenCalledWith("mention_search", { request });
    expect(result.results[0].mention).toEqual(
      expect.objectContaining({ kind: "map.location", locationId: 17 }),
    );
  });
});

describe("inbound events", () => {
  it("dispatches an agent_event push delivered through listen", async () => {
    const { invoke, listen, listeners } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") return { compiling: false, project: "map.scx" };
      if (command === "list") return { files: [] };
      return undefined;
    });
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    await client.connect();
    listeners.get("agent_event")?.({
      payload: { sessionId: "session-a", kind: "reasoning", detail: "checking" },
    });

    expect(received).toContainEqual({
      type: "agent_event",
      sessionId: "session-a",
      kind: "reasoning",
      detail: "checking",
    });
  });

  it("dispatches a git turn-boundary commit for the addressed session", async () => {
    const { invoke, listen, listeners } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (message) => received.push(message),
    });

    await client.connect();
    listeners.get("git")?.({
      payload: {
        sessionId: "session-a",
        external: { sha: "a".repeat(40), subject: "앱 밖 변경", files: 2 },
        turn: { sha: "b".repeat(40), subject: "마린 체력 조정", files: 1 },
      },
    });

    expect(received).toContainEqual({
      type: "git",
      sessionId: "session-a",
      external: { sha: "a".repeat(40), subject: "앱 밖 변경", files: 2 },
      turn: { sha: "b".repeat(40), subject: "마린 체력 조정", files: 1 },
    });
  });

  it("dispatches a structured ASK request for the addressed session", async () => {
    const { invoke, listen, listeners } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (message) => received.push(message),
    });

    await client.connect();
    listeners.get("ask")?.({
      payload: {
        sessionId: "session-a",
        requestId: "ask-1",
        questions: [
          {
            id: "mode",
            question: "방식을 고르세요.",
            multi: false,
            options: [{ label: "A" }, { label: "B" }],
          },
        ],
      },
    });

    expect(received).toContainEqual({
      type: "ask",
      sessionId: "session-a",
      requestId: "ask-1",
      questions: [
        {
          id: "mode",
          question: "방식을 고르세요.",
          multi: false,
          options: [{ label: "A" }, { label: "B" }],
        },
      ],
    });
  });

  it("dispatches typed context usage for the addressed session", async () => {
    const { invoke, listen, listeners } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (message) => received.push(message),
    });
    const tokenUsage = {
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
    };

    await client.connect();
    listeners.get("context_usage")?.({
      payload: {
        sessionId: "session-b",
        turnId: "turn-2",
        tokenUsage,
      },
    });

    expect(received).toContainEqual({
      type: "context_usage",
      sessionId: "session-b",
      turnId: "turn-2",
      tokenUsage,
    });
  });
});

describe("request/response messages", () => {
  it("surfaces status and list invoke results as server messages", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") return { compiling: false, project: "map.scx" };
      if (command === "list") {
        return {
          files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
        };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    await client.connect();
    // connect() only registers listeners; the snapshot is an explicit refresh
    // (App calls it after the first-run setup check).
    expect(received).toEqual([]);
    await client.refresh();

    expect(received).toContainEqual({
      type: "status",
      compiling: false,
      project: "map.scx",
    });
    expect(received).toContainEqual({
      type: "list",
      files: [{ path: "main.eps", ftype: "CUIEps", settable: true }],
    });
  });

  it("treats a no-project list error as state, not a logged failure", async () => {
    // Editor is up (status ok) but no project is open: `list` returns the
    // contractual "ERROR: no project". This must NOT log "IPC command failed
    // (list)"; instead it dispatches a list{error} so the store gates send and
    // the header chip reads "프로젝트 없음".
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") return { compiling: false, project: "" };
      if (command === "list") throw new Error("ERROR: no project");
      return undefined;
    });
    const received: ServerMessage[] = [];
    const logs: { kind: string; text: string }[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
      onLog: (kind, text) => logs.push({ kind, text }),
    });

    await client.connect();
    expect(await client.refresh()).toBe(true);

    expect(
      logs.find((l) => l.text.includes("IPC command failed (list)")),
    ).toBeUndefined();
    expect(received).toContainEqual({
      type: "list",
      error: "ERROR: no project",
    });
  });

  it("still logs a genuine (non-no-project) list failure", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") return { compiling: false, project: "map.scx" };
      if (command === "list") throw new Error("bridge timeout");
      return undefined;
    });
    const logs: { kind: string; text: string }[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
      onLog: (kind, text) => logs.push({ kind, text }),
    });

    await client.connect();
    await client.refresh();

    expect(
      logs.some((l) =>
        l.text.includes("IPC command failed (list): bridge timeout"),
      ),
    ).toBe(true);
  });
});

describe("readiness", () => {
  it("opens transport independently from the native project snapshot", async () => {
    const { invoke, listen } = makeHarness();
    const status = deferred<{ compiling: boolean; project: string }>();
    const list = deferred<{ files: [] }>();
    invoke.mockImplementation((command: string) => {
      if (command === "status") return status.promise;
      if (command === "list") return list.promise;
      return Promise.resolve(undefined);
    });
    const openChanges: boolean[] = [];
    const projectChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
      onOpenChange: (open) => openChanges.push(open),
      onProjectAvailabilityChange: (available) => projectChanges.push(available),
    });

    await client.connect();
    expect(listen).toHaveBeenCalled();
    // Transport opens when listeners register; project validation is separate.
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    expect(projectChanges).toEqual([]);

    const refreshing = client.refresh();
    await flushMicrotasks();
    // Availability resolves only after status and the edge-driven source list.
    expect(projectChanges).toEqual([]);

    status.resolve({ compiling: false, project: "map.scx" });
    await flushMicrotasks();
    expect(projectChanges).toEqual([]);

    list.resolve({ files: [] });
    expect(await refreshing).toBe(true);
    expect(projectChanges).toEqual([true]);
    // refresh() never re-touches the transport.
    expect(openChanges).toEqual([true]);

    const listenCalls = listen.mock.calls.length;
    vi.advanceTimersByTime(10_000);
    await flushMicrotasks();
    // No transport-level reconnect loop: listeners are registered exactly once.
    expect(listen).toHaveBeenCalledTimes(listenCalls);
  });

  it("keeps transport open while a native project disappears and later recovers", async () => {
    const { invoke, listen, listeners, unlisteners } = makeHarness();
    let projectUp = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") {
        if (!projectUp) throw new Error("native project unavailable");
        return { compiling: false, project: "map.scx" };
      }
      if (command === "list") {
        if (!projectUp) throw new Error("native project unavailable");
        return { files: [] };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const openChanges: boolean[] = [];
    const projectChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
      onOpenChange: (open) => openChanges.push(open),
      onProjectAvailabilityChange: (available) => projectChanges.push(available),
    });

    await client.connect();
    // The first native status failure gates project work, not the transport.
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    expect(await client.refresh()).toBe(false);
    expect(projectChanges).toEqual([false]);
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    for (const unlisten of unlisteners) {
      expect(unlisten).not.toHaveBeenCalled();
    }

    // Bootstrap push events still flow while project storage is unavailable.
    listeners.get("progress")?.({
      payload: { stage: "bootstrap", pct: 10, detail: "downloading rag index" },
    });
    expect(received).toContainEqual({
      type: "progress",
      stage: "bootstrap",
      pct: 10,
      detail: "downloading rag index",
    });

    // Repeated failures are edge-suppressed; a later successful refresh recovers.
    expect(await client.refresh()).toBe(false);
    expect(projectChanges).toEqual([false]);

    projectUp = true;
    expect(await client.refresh()).toBe(true);
    expect(projectChanges).toEqual([false, true]);
    expect(openChanges).toEqual([true]);
    expect(received).toContainEqual({
      type: "status",
      compiling: false,
      project: "map.scx",
    });
  });
});

const setupProviders = [
  "codex",
  "claude-code",
  "antigravity",
  "opencode-go",
  "ollama",
].map((provider, index) => ({
  provider,
  availability: index === 0 ? "ready" : "unavailable",
  selectedAsDefault: index === 0,
  canInstall: index < 2,
  canImport: index < 2,
  experimental: provider === "antigravity",
}));

describe("setup commands", () => {
  it("dispatches the setup_status response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    const nullableProviders = setupProviders.map((status) => ({
      ...status,
      detailCode: null,
    }));
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_status") {
        return {
          projectPath: "",
          projectValid: false,
          euddraftPath: "",
          euddraftValid: false,
          assetsReady: false,
          defaultProvider: null,
          providers: nullableProviders,
          projectOpened: false,
          setupRequired: true,
          error: null,
        };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    await client.send({ type: "setup_status" });

    expect(invoke).toHaveBeenCalledWith("setup_status", {});
    expect(received).toContainEqual({
      type: "setup",
      projectPath: "",
      projectValid: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      defaultProvider: null,
      providers: nullableProviders,
      projectOpened: false,
      setupRequired: true,
      error: null,
    });
  });

  it("dispatches the native project picker response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_pick_project_path") {
        return {
          projectPath: "C:\\Projects\\NotNative",
          projectValid: false,
          euddraftPath: "",
          euddraftValid: false,
          assetsReady: false,
          providers: setupProviders,
          projectOpened: false,
          setupRequired: true,
          error: "invalid_project_folder",
        };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    await client.send({ type: "setup_pick_project_path" });

    expect(received).toContainEqual({
      type: "setup",
      projectPath: "C:\\Projects\\NotNative",
      projectValid: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      providers: setupProviders,
      projectOpened: false,
      setupRequired: true,
      error: "invalid_project_folder",
    });
  });

  it("dispatches the project creation response as a setup message", async () => {
      const response = {
        projectPath: "C:\\Projects\\Native",
        projectValid: true,
        euddraftPath: "C:\\Tools\\euddraft.exe",
        euddraftValid: true,
        assetsReady: true,
        defaultProvider: "codex",
        providers: setupProviders,
        projectOpened: true,
        setupRequired: false,
      };
      const { invoke, listen } = makeHarness();
      invoke.mockResolvedValue(response);
      const received: ServerMessage[] = [];
      const client = new IpcClient({
        invoke,
        listen,
        onMessage: (message) => received.push(message),
      });

      await client.send({ type: "setup_create_project" });

      expect(invoke).toHaveBeenCalledWith("setup_create_project", {});
      expect(received).toContainEqual({ type: "setup", ...response });
  });



  it("dispatches the euddraft picker response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_pick_euddraft_path") {
        return {
          projectPath: "C:\\Projects\\Native",
          projectValid: true,
          euddraftPath: "C:\\Tools\\NotEuddraft.exe",
          euddraftValid: false,
          assetsReady: false,
          providers: setupProviders,
          projectOpened: false,
          setupRequired: true,
          error: "invalid_euddraft_path",
        };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    await client.send({ type: "setup_pick_euddraft_path" });

    expect(received).toContainEqual({
      type: "setup",

      projectPath: "C:\\Projects\\Native",
      projectValid: true,
      euddraftPath: "C:\\Tools\\NotEuddraft.exe",
      euddraftValid: false,
      assetsReady: false,
      providers: setupProviders,
      projectOpened: false,
      setupRequired: true,
      error: "invalid_euddraft_path",
    });
  });
  it("returns a typed E3S export path and preserves dialog cancellation", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValueOnce({ path: "C:\\Exports\\demo.e3s" })
      .mockResolvedValueOnce(null);

    await expect(projectExportE3s(invoke)).resolves.toEqual({
      path: "C:\\Exports\\demo.e3s",
    });
    await expect(projectExportE3s(invoke)).resolves.toBeNull();
    expect(invoke).toHaveBeenNthCalledWith(1, "project_export_e3s");
    expect(invoke).toHaveBeenNthCalledWith(2, "project_export_e3s");
  });

  it("sends folder selection intent and dispatches latest install snapshots", async () => {
    const response = {
      projectPath: "C:\\Projects\\Native",
      projectValid: true,
      euddraftPath: "C:\\euddraft\\euddraft.exe",
      euddraftValid: true,
      assetsReady: true,
      providers: setupProviders,
      projectOpened: false,
      setupRequired: true,
    };
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(response);
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (message) => received.push(message),
    });

    await client.send({ type: "setup_pick_euddraft_path", directory: true });
    await client.send({ type: "setup_install_euddraft" });

    expect(invoke).toHaveBeenNthCalledWith(1, "setup_pick_euddraft_path", {
      directory: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "setup_install_euddraft", {});
    expect(received).toHaveLength(2);
    expect(received[1]).toEqual({ type: "setup", ...response });
  });

  it("sends bootstrap_run without expecting a response payload", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const received: ServerMessage[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
    });

    expect(await client.send({ type: "bootstrap_run" })).toBe(true);

    expect(invoke).toHaveBeenCalledWith("bootstrap_run", {});
    expect(received).toEqual([]);
  });
});

describe("provider model settings commands", () => {
  const model = {
    provider: "codex",
    model: "gpt-5.5-codex",
    displayName: "GPT-5.5 Codex",
    description: "Most capable",
    isDefault: true,
    capabilities: {
      vision: true,
      toolCalls: true,
      strictStructuredOutput: true,
      reasoningLevels: ["high"],
      nativeCompaction: true,
      hostedWebSearch: true,
    },
  };
  const status = {
    provider: "codex",
    availability: "ready",
    selectedAsDefault: true,
    canInstall: true,
    canImport: true,
    experimental: false,
  };
  const response = {
    provider: "codex",
    status,
    models: [model],
    selectedModel: "gpt-5.5-codex",
    selectedReasoning: { level: "high" },
    hasApiKey: false,
  };

  it("fetches one provider settings view", async () => {
    const invoke = vi.fn().mockResolvedValue(response);
    await expect(providerSettingsGet("codex", invoke)).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("provider_settings", {
      provider: "codex",
    });
  });

  it("saves an Ollama OpenAI-compatible base URL", async () => {
    const ollamaResponse = {
      ...response,
      provider: "ollama",
      status: { ...status, provider: "ollama" },
      models: [],
      selectedModel: null,
      selectedReasoning: null,
      baseUrl: "https://ollama.example.test/v1",
      hasApiKey: true,
    };
    const invoke = vi.fn().mockResolvedValue(ollamaResponse);
    await expect(
      providerBaseUrlSave(
        "ollama",
        "https://ollama.example.test/v1",
        invoke,
      ),
    ).resolves.toEqual(ollamaResponse);
    expect(invoke).toHaveBeenCalledWith("provider_base_url_save", {
      provider: "ollama",
      baseUrl: "https://ollama.example.test/v1",
    });
  });

  it("saves provider defaults without changing existing sessions", async () => {
    const invoke = vi.fn().mockResolvedValue(response);
    await expect(
      providerDefaultsSave(
        "codex",
        "gpt-5.5-codex",
        { level: "high" },
        true,
        invoke,
      ),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("provider_defaults_save", {
      provider: "codex",
      model: "gpt-5.5-codex",
      reasoning: { level: "high" },
      setDefaultProvider: true,
    });
  });

  it("normalizes Rust null reasoning when loading session settings", async () => {
    const invoke = vi.fn().mockResolvedValue({
      provider: "antigravity",
      models: [{ ...model, provider: "antigravity" }],
      selectedModel: "gemini-3.7-flash-high",
      selectedReasoning: null,
    });
    await expect(sessionModelSettingsGet("session-1", invoke)).resolves.toEqual({
      provider: "antigravity",
      models: [{ ...model, provider: "antigravity" }],
      selectedModel: "gemini-3.7-flash-high",
      selectedReasoning: undefined,
    });
  });

  it("saves model settings against one bound session", async () => {
    const sessionResponse = {
      provider: "codex",
      models: [model],
      selectedModel: "gpt-5.5-codex",
      selectedReasoning: { level: "high" },
    };
    const invoke = vi.fn().mockResolvedValue(sessionResponse);
    await expect(
      sessionModelSettingsSave(
        "session-1",
        "gpt-5.5-codex",
        { level: "high" },
        invoke,
      ),
    ).resolves.toEqual(sessionResponse);
    expect(invoke).toHaveBeenCalledWith("session_model_settings_save", {
      sessionId: "session-1",
      model: "gpt-5.5-codex",
      reasoning: { level: "high" },
    });
  });
});

describe("App notification settings commands", () => {
  const settings = {
    notifications: {
      planApproval: { sound: true, osNotification: true },
      reviewRequired: { sound: true, osNotification: true },
      agentTurnComplete: { sound: true, osNotification: false },
      askResponseRequired: { sound: false, osNotification: true },
    },
    codexLargeContextModels: ["gpt-5.5-codex"],
    deepPlanning: true,
    scmdraftPath: String.raw`C:\Tools\ScmDraft 2\ScmDraft 2.exe`,
  };

  it("loads and saves the complete app settings payload", async () => {
    const invoke = vi.fn().mockResolvedValue(settings);

    await expect(appSettingsGet(invoke)).resolves.toEqual(settings);
    expect(invoke).toHaveBeenCalledWith("app_settings");

    invoke.mockClear();
    await expect(appSettingsSave(settings, invoke)).resolves.toEqual(settings);
    expect(invoke).toHaveBeenCalledWith("app_settings_save", { settings });
  });

  it("fills a missing SCMDraft path with an empty string and rejects a non-string one", async () => {
    const { scmdraftPath: _omitted, ...legacy } = settings;
    await expect(appSettingsGet(vi.fn().mockResolvedValue(legacy))).resolves.toEqual({
      ...legacy,
      scmdraftPath: "",
    });
    await expect(
      appSettingsGet(vi.fn().mockResolvedValue({ ...legacy, scmdraftPath: 7 })),
    ).rejects.toThrow("invalid app settings response");
  });

  it("reports whether SCMDraft 2 launched or still needs its executable", async () => {
    const launched = vi.fn().mockResolvedValue({ kind: "launched" });
    await expect(openScmdraft(launched)).resolves.toEqual({ kind: "launched" });
    expect(launched).toHaveBeenCalledWith("project_open_scmdraft");
    await expect(openScmdraft(vi.fn().mockResolvedValue({ kind: "unconfigured" }))).resolves.toEqual({
      kind: "unconfigured",
    });
    await expect(openScmdraft(vi.fn().mockResolvedValue({ kind: "later" }))).rejects.toThrow(
      "invalid scmdraft launch response",
    );
    await expect(openScmdraft(vi.fn().mockResolvedValue(null))).rejects.toThrow(
      "invalid scmdraft launch response",
    );
  });

  it("bounds a build report to the fields the dialog renders", async () => {
    const invoke = vi.fn().mockResolvedValue({
      ok: false,
      errors: [
        {
          source: "epscript",
          file: "src/main.eps",
          line: 12,
          message: "unknown name foo",
          raw: "[Error 1] Module \"main\" Line 12 : unknown name foo",
          count: 1,
        },
      ],
      warnings: [],
      rawStatus: 1,
      outputExcerpt: "euddraft 0.9.9",
      outputMap: "build/[EUD]project.scx",
      deployedMap: null,
      logPath: "C:\map\build\euddraft\build.log",
      // A field the core adds later must not break the view.
      buildProgress: { consecutiveNoProgress: 0 },
    });
    const report = await runProjectBuild(invoke);
    expect(invoke).toHaveBeenCalledWith("project_build_run");
    expect(report.ok).toBe(false);
    expect(report.errors[0]).toEqual({
      source: "epscript",
      file: "src/main.eps",
      line: 12,
      message: "unknown name foo",
      raw: "[Error 1] Module \"main\" Line 12 : unknown name foo",
      count: 1,
    });
    expect(report.deployedMap).toBeNull();
    expect(report.logPath).toBe("C:\map\build\euddraft\build.log");
    expect("buildProgress" in report).toBe(false);

    await expect(
      runProjectBuild(vi.fn().mockResolvedValue({ ok: true })),
    ).rejects.toThrow("invalid project build response");
    await expect(
      runProjectBuild(
        vi.fn().mockResolvedValue({
          ok: true,
          errors: [{ source: "s", file: "f", line: 1, message: "m" }],
          warnings: [],
          rawStatus: 0,
          outputExcerpt: "",
          outputMap: "out.scx",
        }),
      ),
    ).rejects.toThrow("invalid project build error");
  });

  it("opens the project root in the requested external tool", async () => {
    const invoke = vi.fn().mockResolvedValue(null);
    await openProjectRootIn("vscode", invoke);
    await openProjectRootIn("fileManager", invoke);
    expect(invoke).toHaveBeenNthCalledWith(1, "project_open_root_in", { target: "vscode" });
    expect(invoke).toHaveBeenNthCalledWith(2, "project_open_root_in", { target: "fileManager" });
  });

  it("returns the picked SCMDraft path or null when the picker is cancelled", async () => {
    const picked = vi.fn().mockResolvedValue(String.raw`C:\Tools\ScmDraft 2\ScmDraft 2.exe`);
    await expect(pickScmdraftPath(picked)).resolves.toBe(String.raw`C:\Tools\ScmDraft 2\ScmDraft 2.exe`);
    expect(picked).toHaveBeenCalledWith("settings_pick_scmdraft_path");
    await expect(pickScmdraftPath(vi.fn().mockResolvedValue(null))).resolves.toBeNull();
    await expect(pickScmdraftPath(vi.fn().mockResolvedValue(3))).rejects.toThrow(
      "invalid scmdraft path response",
    );
  });

  it("loads, checks, and updates the typed euddraft settings contract", async () => {
    const response = {
      path: String.raw`C:\euddraft\euddraft.exe`,
      valid: true,
      managed: true,
      installedVersion: "v0.10.2.5",
      latestVersion: "v0.11.0.1",
      updateAvailable: true,
    };
    const invoke = vi.fn().mockResolvedValue(response);

    await expect(euddraftSettingsGet(invoke)).resolves.toEqual(response);
    expect(invoke).toHaveBeenLastCalledWith("euddraft_settings");
    await expect(euddraftCheckUpdate(invoke)).resolves.toEqual(response);
    expect(invoke).toHaveBeenLastCalledWith("euddraft_check_update");
    await expect(euddraftUpdate(invoke)).resolves.toEqual(response);
    expect(invoke).toHaveBeenLastCalledWith("euddraft_update");
  });

  it("rejects malformed notification channel settings", async () => {
    const invoke = vi.fn().mockResolvedValue({
      notifications: {
        planApproval: { sound: true },
        agentTurnComplete: { sound: true, osNotification: true },
        askResponseRequired: { sound: true, osNotification: true },
      },
      codexLargeContextModels: [],
      deepPlanning: false,
    });

    await expect(appSettingsGet(invoke)).rejects.toThrow(
      "invalid app settings response",
    );
  });

  it("invokes native compaction for the named session", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);

    await compactSession("session-a", invoke);

    expect(invoke).toHaveBeenCalledWith("compact", {
      sessionId: "session-a",
    });
  });

  it("delivers attention events with focus and session context", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);

    await attentionNotify("planApproval", false, "session-a", invoke);
    expect(invoke).toHaveBeenCalledWith("attention_notify", {
      kind: "planApproval",
      showOs: false,
      sessionId: "session-a",
    });

    await attentionNotify("agentTurnComplete", true, "session-a", invoke);
    expect(invoke).toHaveBeenLastCalledWith("attention_notify", {
      kind: "agentTurnComplete",
      showOs: true,
      sessionId: "session-a",
    });

    await attentionNotify("askResponseRequired", false, "session-b", invoke);
    expect(invoke).toHaveBeenLastCalledWith("attention_notify", {
      kind: "askResponseRequired",
      showOs: false,
      sessionId: "session-b",
    });

    await notificationSoundPreview(invoke);
    expect(invoke).toHaveBeenLastCalledWith("notification_sound_preview");
  });

  it("classifies only ordinary settled agent turns as completion notifications", () => {
    expect(isAgentTurnEndTransition("running_read", "idle")).toBe(true);
    expect(isAgentTurnEndTransition("running_write", "error")).toBe(true);
    expect(isAgentTurnEndTransition("running_read", "review")).toBe(false);
    expect(isAgentTurnEndTransition("waiting_input", "idle")).toBe(false);
    expect(isAgentTurnEndTransition("idle", "idle")).toBe(false);
  });
});

describe("Workspace commands", () => {
  const workspace = {
    project: "Example",
    workspaceId: "a".repeat(64),
    files: [{ path: "specs/game.md", size: 12 }],
  };

  it("lists the current project workspace", async () => {
    const invoke = vi.fn().mockResolvedValue(workspace);
    await expect(workspaceList(invoke)).resolves.toEqual(workspace);
    expect(invoke).toHaveBeenCalledWith("workspace_list");
  });

  it("reads a confined project file by id and project-relative path", async () => {
    const response = {
      workspaceId: workspace.workspaceId,
      path: "src/main.eps",
      size: 6,
      content: "# Game",
    };
    const invoke = vi.fn().mockResolvedValue(response);
    await expect(
      workspaceRead(workspace.workspaceId, "src/main.eps", invoke),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("workspace_read", {
      workspaceId: workspace.workspaceId,
      path: "src/main.eps",
    });
  });

  it("accepts a closed binary/oversized file and rejects an inconsistent read", async () => {
    const binary = {
      workspaceId: workspace.workspaceId,
      path: "maps/source.scx",
      size: 2048,
      content: null,
      unreadable: "binary",
    };
    await expect(
      workspaceRead(workspace.workspaceId, "maps/source.scx", vi.fn().mockResolvedValue(binary)),
    ).resolves.toEqual(binary);
    for (const broken of [
      { ...binary, unreadable: undefined },
      { ...binary, content: "x" },
      { ...binary, unreadable: "encrypted" },
      { ...binary, size: "2048" },
    ]) {
      await expect(
        workspaceRead(workspace.workspaceId, "maps/source.scx", vi.fn().mockResolvedValue(broken)),
      ).rejects.toThrow("invalid workspace read response");
    }
  });

  it("searches workspace filenames and text content through one command", async () => {
    const response = {
      workspaceId: workspace.workspaceId,
      query: "confirmed behavior",
      paths: ["specs/game.md"],
    };
    const invoke = vi.fn().mockResolvedValue(response);

    await expect(
      workspaceSearch(workspace.workspaceId, response.query, invoke),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("workspace_search", {
      workspaceId: workspace.workspaceId,
      query: response.query,
    });
  });

  it("rejects malformed file entries", async () => {
    const invoke = vi.fn().mockResolvedValue({
      ...workspace,
      files: [{ path: "specs/game.md", size: "twelve" }],
    });
    await expect(workspaceList(invoke)).rejects.toThrow(
      "invalid workspace file entry",
    );
  });
});

describe("project history commands", () => {
  it("normalizes the repository state, including a missing origin", async () => {
    const invoke = vi.fn().mockResolvedValue({
      available: true,
      tracked: true,
      nested: false,
      origin: null,
      consent: "pending",
      warning: null,
    });

    await expect(gitState(invoke)).resolves.toEqual({
      available: true,
      tracked: true,
      nested: false,
      origin: null,
      consent: "pending",
      warning: null,
    });
    expect(invoke).toHaveBeenCalledWith("git_state");
  });

  it("rejects a repository state with an unknown consent value", async () => {
    const invoke = vi.fn().mockResolvedValue({
      available: true,
      tracked: true,
      nested: false,
      origin: "app",
      consent: "maybe",
    });

    await expect(gitState(invoke)).rejects.toThrow("invalid git state response");
  });

  it("records consent and returns the updated state", async () => {
    const invoke = vi.fn().mockResolvedValue({
      available: true,
      tracked: true,
      nested: false,
      origin: "preexisting",
      consent: "granted",
      warning: null,
    });

    await expect(gitConsentSet(true, invoke)).resolves.toMatchObject({
      consent: "granted",
      origin: "preexisting",
    });
    expect(invoke).toHaveBeenCalledWith("git_consent_set", { granted: true });
  });

  it("reads the log, a commit detail, and a revert", async () => {
    const invoke = vi.fn().mockImplementation(async (command: string) => {
      if (command === "git_log") {
        return [{ sha: "a".repeat(40), subject: "첫 변경", timestamp: 1_700_000_000 }];
      }
      if (command === "git_commit_detail") {
        return {
          sha: "a".repeat(40),
          subject: "첫 변경",
          body: ["session: s-1", "request: r-1"].join("\n"),
          timestamp: 1_700_000_000,
          files: [
            {
              path: "src/main.eps",
              insertions: 2,
              deletions: 1,
              binary: false,
              patch: ["@@ -1 +1 @@", "-old", "+new"].join("\n"),
            },
            {
              path: "maps/source.scx",
              insertions: 0,
              deletions: 0,
              binary: true,
              omitted: "바이너리 파일이라 내용 비교를 표시하지 않습니다.",
            },
          ],
        };
      }
      return { sha: "c".repeat(40), subject: 'Revert "첫 변경"', files: 1 };
    });

    await expect(gitLog(10, invoke)).resolves.toEqual([
      { sha: "a".repeat(40), subject: "첫 변경", timestamp: 1_700_000_000 },
    ]);
    expect(invoke).toHaveBeenLastCalledWith("git_log", { limit: 10 });

    const detail = await gitCommitDetail("a".repeat(40), invoke);
    expect(detail.files[0]?.patch).toContain("+new");
    // An omitted patch stays absent rather than becoming an empty diff.
    expect(detail.files[1]?.patch).toBeUndefined();
    expect(detail.files[1]?.omitted).toContain("바이너리");

    await expect(gitRevert("a".repeat(40), invoke)).resolves.toEqual({
      sha: "c".repeat(40),
      subject: 'Revert "첫 변경"',
      files: 1,
    });
    expect(invoke).toHaveBeenLastCalledWith("git_revert", { sha: "a".repeat(40) });
  });
});
