import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { IpcClient } from "@/lib/ipc";
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
    const msg: ClientMessage = { type: "chat", text: "hello" };

    await client.send(msg);

    expect(invoke).toHaveBeenCalledWith("chat", { text: "hello" });
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
      decision: "reject",
      ids: ["a", "b"],
    };

    await client.send(msg);

    expect(invoke).toHaveBeenCalledWith("changeset_decision", {
      decision: "reject",
      ids: ["a", "b"],
    });
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
      payload: { kind: "reasoning", detail: "checking" },
    });

    expect(received).toContainEqual({
      type: "agent_event",
      kind: "reasoning",
      detail: "checking",
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
});

describe("readiness", () => {
  it("reports open after listeners register and initial status + list resolve, without a reconnect loop", async () => {
    const { invoke, listen } = makeHarness();
    const status = deferred<{ compiling: boolean; project: string }>();
    const list = deferred<{ files: [] }>();
    invoke.mockImplementation((command: string) => {
      if (command === "status") return status.promise;
      if (command === "list") return list.promise;
      return Promise.resolve(undefined);
    });
    const openChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: () => {},
      onOpenChange: (open) => openChanges.push(open),
    });

    await client.connect();
    expect(listen).toHaveBeenCalled();
    // Listeners alone never report readiness — that needs the snapshot.
    expect(openChanges).toEqual([]);

    const refreshing = client.refresh();
    await flushMicrotasks();
    expect(openChanges).toEqual([]);

    status.resolve({ compiling: false, project: "map.scx" });
    await flushMicrotasks();
    expect(openChanges).toEqual([]);

    list.resolve({ files: [] });
    await refreshing;
    expect(openChanges).toEqual([true]);

    const listenCalls = listen.mock.calls.length;
    const invokeCalls = invoke.mock.calls.length;
    vi.advanceTimersByTime(10_000);
    await flushMicrotasks();

    expect(listen).toHaveBeenCalledTimes(listenCalls);
    expect(invoke).toHaveBeenCalledTimes(invokeCalls);
  });

  it("keeps push listeners alive when a refresh fails, and a later refresh() recovers", async () => {
    // Fallback path (EUD-132): a refresh against an unconfigured/parked app
    // fails without tearing listeners down — bootstrap progress must still
    // arrive, and refresh() succeeds after setup completes.
    const { invoke, listen, listeners, unlisteners } = makeHarness();
    let setupDone = false;
    invoke.mockImplementation(async (command: string) => {
      if (command === "status") {
        if (!setupDone) throw new Error("editor path not configured");
        return { compiling: false, project: "map.scx" };
      }
      if (command === "list") {
        if (!setupDone) throw new Error("editor path not configured");
        return { files: [] };
      }
      return undefined;
    });
    const received: ServerMessage[] = [];
    const openChanges: boolean[] = [];
    const client = new IpcClient({
      invoke,
      listen,
      onMessage: (m) => received.push(m),
      onOpenChange: (open) => openChanges.push(open),
    });

    await client.connect();
    expect(await client.refresh()).toBe(false);

    expect(openChanges).toEqual([false]);
    expect(client.isOpen()).toBe(false);
    for (const unlisten of unlisteners) {
      expect(unlisten).not.toHaveBeenCalled();
    }

    // Push events still flow while not "open".
    listeners.get("progress")?.({
      payload: { stage: "bootstrap", pct: 10, detail: "downloading rag index" },
    });
    expect(received).toContainEqual({
      type: "progress",
      stage: "bootstrap",
      pct: 10,
      detail: "downloading rag index",
    });

    setupDone = true;
    expect(await client.refresh()).toBe(true);
    expect(client.isOpen()).toBe(true);
    expect(openChanges).toEqual([false, true]);
    expect(received).toContainEqual({
      type: "status",
      compiling: false,
      project: "map.scx",
    });
  });
});

describe("setup commands", () => {
  it("dispatches the setup_status response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_status") {
        return {
          editor_path: "",
          editor_valid: false,
          assets_ready: false,
          codex_resolved: true,
          codex_authed: false,
          setup_required: true,
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
      editor_path: "",
      editor_valid: false,
      assets_ready: false,
      codex_resolved: true,
      codex_authed: false,
      setup_required: true,
    });
  });

  it("dispatches the setup_pick_editor_path response as a setup message", async () => {
    const { invoke, listen } = makeHarness();
    invoke.mockImplementation(async (command: string) => {
      if (command === "setup_pick_editor_path") {
        return {
          editor_path: "C:\\Games\\NotTheEditor",
          editor_valid: false,
          assets_ready: false,
          codex_resolved: true,
          codex_authed: false,
          setup_required: true,
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
      editor_path: "C:\\Games\\NotTheEditor",
      editor_valid: false,
      assets_ready: false,
      codex_resolved: true,
      codex_authed: false,
      setup_required: true,
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
