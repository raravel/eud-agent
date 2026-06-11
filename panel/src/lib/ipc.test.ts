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
