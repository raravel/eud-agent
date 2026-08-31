import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  IpcClient,
  appSettingsGet,
  appSettingsSave,
  attentionNotify,
  providerBaseUrlSave,
  providerDefaultsSave,
  providerSettingsGet,
  sessionModelSettingsGet,
  sessionModelSettingsSave,
  compactSession,
  isAgentTurnEndTransition,
  notificationSoundPreview,
  mentionSearch,
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

  it("sends changeset_decision via invoke", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockResolvedValue(undefined);
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
    });
    const msg: ClientMessage = {
      type: "changeset_decision",
      sessionId: "session-a",
      decision: "reject",
      ids: ["a", "b"],
    };

    await client.send(msg);

    expect(invoke).toHaveBeenCalledWith("changeset_decision", {
      sessionId: "session-a",
      decision: "reject",
      ids: ["a", "b"],
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
  it("opens the transport on listener registration, independent of the editor snapshot, without a reconnect loop", async () => {
    const { invoke, listen } = makeHarness();
    const status = deferred<{ compiling: boolean; project: string }>();
    const list = deferred<{ files: [] }>();
    invoke.mockImplementation((command: string) => {
      if (command === "status") return status.promise;
      if (command === "list") return list.promise;
      return Promise.resolve(undefined);
    });
    const openChanges: boolean[] = [];
    const editorChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
      onOpenChange: (open) => openChanges.push(open),
      onEditorChange: (connected) => editorChanges.push(connected),
    });

    await client.connect();
    expect(listen).toHaveBeenCalled();
    // Transport open the moment listeners register — NOT gated on the editor.
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    expect(editorChanges).toEqual([]);

    const refreshing = client.refresh();
    await flushMicrotasks();
    // The editor edge fires only once both the status probe and the edge-driven
    // list round-trip resolve.
    expect(editorChanges).toEqual([]);

    status.resolve({ compiling: false, project: "map.scx" });
    await flushMicrotasks();
    expect(editorChanges).toEqual([]);

    list.resolve({ files: [] });
    expect(await refreshing).toBe(true);
    expect(editorChanges).toEqual([true]);
    // refresh() never re-touches the transport.
    expect(openChanges).toEqual([true]);

    const listenCalls = listen.mock.calls.length;
    vi.advanceTimersByTime(10_000);
    await flushMicrotasks();
    // No transport-level reconnect loop: listeners are registered exactly once.
    expect(listen).toHaveBeenCalledTimes(listenCalls);
  });

  it("treats a failed editor probe as editor-down (transport stays open) and recovers on a later refresh()", async () => {
    // The editor heartbeat being stale/absent must NOT read as a dead transport:
    // listeners stay alive (bootstrap progress still flows), the transport stays
    // open, and only editor liveness flips — recovering automatically when a
    // later poll succeeds.
    const { invoke, listen, listeners, unlisteners } = makeHarness();
    let editorUp = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") {
        if (!editorUp) throw new Error("editor not connected");
        return { compiling: false, project: "map.scx" };
      }
      if (command === "list") {
        if (!editorUp) throw new Error("editor not connected");
        return { files: [] };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const openChanges: boolean[] = [];
    const editorChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
      onOpenChange: (open) => openChanges.push(open),
      onEditorChange: (connected) => editorChanges.push(connected),
    });

    await client.connect();
    // Transport open from connect; the first editor probe fails -> editor-down.
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    expect(await client.refresh()).toBe(false);
    expect(editorChanges).toEqual([false]);
    // Transport untouched by the editor probe; listeners intact.
    expect(openChanges).toEqual([true]);
    expect(client.isOpen()).toBe(true);
    for (const unlisten of unlisteners) {
      expect(unlisten).not.toHaveBeenCalled();
    }

    // Push events still flow while the editor is down.
    listeners.get("progress")?.({
      payload: { stage: "bootstrap", pct: 10, detail: "downloading rag index" },
    });
    expect(received).toContainEqual({
      type: "progress",
      stage: "bootstrap",
      pct: 10,
      detail: "downloading rag index",
    });

    // A steady-state repeat failure stays quiet (no edge) — the poll never spams.
    expect(await client.refresh()).toBe(false);
    expect(editorChanges).toEqual([false]);

    editorUp = true;
    expect(await client.refresh()).toBe(true);
    expect(editorChanges).toEqual([false, true]);
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
          editorPath: "",
          editorValid: false,
          assetsReady: false,
          defaultProvider: null,
          providers: nullableProviders,
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
      editorPath: "",
      editorValid: false,
      assetsReady: false,
      defaultProvider: null,
      providers: nullableProviders,
      setupRequired: true,
      error: null,
    });
  });

  it("dispatches the setup_pick_editor_path response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_pick_editor_path") {
        return {
          editorPath: "C:\\Games\\NotTheEditor",
          editorValid: false,
          assetsReady: false,
          providers: setupProviders,
          setupRequired: true,
          error: "invalid_editor_folder",
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

    await client.send({ type: "setup_pick_editor_path" });

    expect(received).toContainEqual({
      type: "setup",
      editorPath: "C:\\Games\\NotTheEditor",
      editorValid: false,
      assetsReady: false,
      providers: setupProviders,
      setupRequired: true,
      error: "invalid_editor_folder",
    });
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
      changesetReview: { sound: false, osNotification: true },
      agentTurnComplete: { sound: true, osNotification: false },
      askResponseRequired: { sound: false, osNotification: true },
    },
    codexLargeContextModels: ["gpt-5.5-codex"],
  };

  it("loads and saves the complete app settings payload", async () => {
    const invoke = vi.fn().mockResolvedValue(settings);

    await expect(appSettingsGet(invoke)).resolves.toEqual(settings);
    expect(invoke).toHaveBeenCalledWith("app_settings");

    invoke.mockClear();
    await expect(appSettingsSave(settings, invoke)).resolves.toEqual(settings);
    expect(invoke).toHaveBeenCalledWith("app_settings_save", { settings });
  });

  it("rejects malformed notification channel settings", async () => {
    const invoke = vi.fn().mockResolvedValue({
      notifications: {
        planApproval: { sound: true },
        changesetReview: { sound: true, osNotification: true },
        agentTurnComplete: { sound: true, osNotification: true },
        askResponseRequired: { sound: true, osNotification: true },
      },
      codexLargeContextModels: [],
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

  it("delivers attention events with focus, session, and item-count context", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined);

    await attentionNotify("planApproval", false, "session-a", undefined, invoke);
    expect(invoke).toHaveBeenCalledWith("attention_notify", {
      kind: "planApproval",
      showOs: false,
      sessionId: "session-a",
    });

    await attentionNotify("changesetReview", true, "session-b", 3, invoke);
    expect(invoke).toHaveBeenLastCalledWith("attention_notify", {
      kind: "changesetReview",
      showOs: true,
      sessionId: "session-b",
      itemCount: 3,
    });

    await attentionNotify(
      "agentTurnComplete",
      true,
      "session-a",
      undefined,
      invoke,
    );
    expect(invoke).toHaveBeenLastCalledWith("attention_notify", {
      kind: "agentTurnComplete",
      showOs: true,
      sessionId: "session-a",
    });

    await attentionNotify(
      "askResponseRequired",
      false,
      "session-b",
      undefined,
      invoke,
    );
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
    files: [{ path: "specs/game.md", source: false, size: 12 }],
  };

  it("lists the current project workspace", async () => {
    const invoke = vi.fn().mockResolvedValue(workspace);
    await expect(workspaceList(invoke)).resolves.toEqual(workspace);
    expect(invoke).toHaveBeenCalledWith("workspace_list");
  });

  it("reads a confined workspace file by id and relative path", async () => {
    const response = {
      workspaceId: workspace.workspaceId,
      path: "specs/game.md",
      source: false,
      content: "# Game",
    };
    const invoke = vi.fn().mockResolvedValue(response);
    await expect(
      workspaceRead(workspace.workspaceId, "specs/game.md", invoke),
    ).resolves.toEqual(response);
    expect(invoke).toHaveBeenCalledWith("workspace_read", {
      workspaceId: workspace.workspaceId,
      path: "specs/game.md",
    });
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
      files: [{ path: "source/main.eps", source: "yes", size: 1 }],
    });
    await expect(workspaceList(invoke)).rejects.toThrow(
      "invalid workspace file entry",
    );
  });
});
