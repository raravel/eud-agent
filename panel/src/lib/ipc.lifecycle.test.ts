import { describe, expect, it, vi } from "vitest";
import { IpcClient, type ListenFn, type ServerMessage } from "./ipc";
import { createPanelStore } from "../state/store";

describe("IPC listener lifetime", () => {
  it("ignores stopped registrations while a replacement streams Korean chunks", async () => {
    const callbacks: Array<(event: { payload: unknown }) => void> = [];
    let finishRegistration: (dispose: () => void) => void = () => {};
    const delayed = new Promise<() => void>((resolve) => { finishRegistration = resolve; });
    const staleDispose = vi.fn();
    let first = true;
    const listen: ListenFn = async (_name, callback) => {
      callbacks.push(callback);
      if (first) { first = false; return delayed; }
      return vi.fn();
    };
    const received: ServerMessage[] = [];
    const previous = new IpcClient({ listen, invoke: vi.fn(), onMessage: (message) => received.push(message) });
    const pending = previous.connect();
    previous.stop();
    const replacement = new IpcClient({ listen, invoke: vi.fn(), onMessage: (message) => received.push(message) });
    await replacement.connect();
    callbacks[0]?.({ payload: { sessionId: "session-a", kind: "reasoning", detail: "맵" } });
    expect(received).toEqual([]);
    for (const detail of ["맵", " 에", "이", "전", "트"]) {
      const event = { payload: { sessionId: "session-a", kind: "reasoning", detail } };
      callbacks[0]?.(event);
      callbacks[1]?.(event);
    }
    expect(received.map((message) => message.type === "agent_event" ? message.detail : "").join("")).toBe("맵 에이전트");
    finishRegistration(staleDispose);
    await pending;
    expect(staleDispose).toHaveBeenCalledOnce();
    replacement.stop();
    callbacks[1]?.({ payload: { sessionId: "session-a", kind: "reasoning", detail: "stopped" } });
    expect(received).toHaveLength(5);
  });

  it("discards a pending connection when the same client reconnects", async () => {
    const callbacks: Array<(event: { payload: unknown }) => void> = [];
    let finishRegistration: (dispose: () => void) => void = () => {};
    const delayed = new Promise<() => void>((resolve) => { finishRegistration = resolve; });
    const staleDispose = vi.fn();
    let first = true;
    const listen: ListenFn = async (_name, callback) => {
      callbacks.push(callback);
      if (first) { first = false; return delayed; }
      return vi.fn();
    };
    const received: ServerMessage[] = [];
    const opened = vi.fn();
    const client = new IpcClient({ listen, invoke: vi.fn(), onMessage: (message) => received.push(message), onOpenChange: opened });
    const pending = client.connect();
    client.stop();
    await client.connect();
    finishRegistration(staleDispose);
    await pending;
    callbacks[0]?.({ payload: { sessionId: "session-a", kind: "reasoning", detail: "맵" } });
    expect(received).toEqual([]);
    expect(staleDispose).toHaveBeenCalledOnce();
    expect(opened).toHaveBeenCalledTimes(1);
    client.stop();
  });

  it("archives streamed text and the final answer once after an effect replacement", async () => {
    const callbacks = new Map<string, Array<(event: { payload: unknown }) => void>>();
    let finishRegistration: (dispose: () => void) => void = () => {};
    let registrationStarted: () => void = () => {};
    const delayed = new Promise<() => void>((resolve) => { finishRegistration = resolve; });
    const started = new Promise<void>((resolve) => { registrationStarted = resolve; });
    let first = true;
    const listen: ListenFn = async (name, callback) => {
      const handlers = callbacks.get(name) ?? [];
      handlers.push(callback);
      callbacks.set(name, handlers);
      if (name === "plan" && first) {
        first = false;
        registrationStarted();
        return delayed;
      }
      return vi.fn();
    };
    const store = createPanelStore();
    store.chatSent();
    const onMessage = (message: ServerMessage) => {
      if (message.type === "agent_event") store.agentEvent(message.kind, message.detail, message.data);
      if (message.type === "answer") store.answerReceived(message.text);
    };
    const previous = new IpcClient({ listen, invoke: vi.fn(), onMessage });
    const pending = previous.connect();
    await started;
    previous.stop();
    const current = new IpcClient({ listen, invoke: vi.fn(), onMessage });
    await current.connect();
    for (const detail of ["맵", " 에", "이", "전", "트"]) {
      for (const handler of callbacks.get("agent_event") ?? []) {
        handler({ payload: { sessionId: "session-a", kind: "delta", detail } });
      }
    }
    for (const handler of callbacks.get("answer") ?? []) {
      handler({ payload: { sessionId: "session-a", text: "맵 에이전트" } });
    }
    expect(store.getState().log.filter((entry) => entry.kind === "agent").map((entry) => entry.text)).toEqual(["맵 에이전트"]);
    finishRegistration(vi.fn());
    await pending;
    current.stop();
  });
});
