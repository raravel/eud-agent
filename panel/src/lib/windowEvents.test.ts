import { beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { EventTarget } from "@tauri-apps/api/event";
import { IpcClient } from "./ipc";
import { createPanelStore } from "../state/store";
import { createMapEventSubscriptions } from "../map/mapEventSubscriptions";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ label: "main" }),
}));

type Registration = {
  readonly event: string;
  readonly target: EventTarget;
  readonly handler: number;
};

function projectTransport() {
  const registrations = new Map<number, Registration>();
  let sequence = 0;
  mockIPC((command, args) => {
    if (command === "plugin:event|listen") {
      sequence += 1;
      if (typeof args.event !== "string" || typeof args.handler !== "number") {
        throw new Error("Invalid event registration");
      }
      const target = args.target;
      if (typeof target !== "object" || target === null || !("kind" in target)) {
        throw new Error("Missing event target");
      }
      if (target.kind !== "Any" && target.kind !== "AnyLabel") {
        throw new Error("Unexpected event target");
      }
      if (target.kind === "AnyLabel" && !("label" in target && typeof target.label === "string")) {
        throw new Error("Missing target label");
      }
      registrations.set(sequence, {
        event: args.event,
        handler: args.handler,
        target: target.kind === "Any" ? { kind: "Any" } : { kind: "AnyLabel", label: String("label" in target ? target.label : "") },
      });
      return sequence;
    }
    if (command === "plugin:event|unlisten") {
      registrations.delete(Number(args.eventId));
      return;
    }
    throw new Error(`Unexpected command: ${command}`);
  });
  return {
    emitTo(label: string | undefined, event: string, payload: unknown) {
      for (const [id, registration] of registrations) {
        if (registration.event !== event) continue;
        const target = registration.target;
        if (label !== undefined && target.kind !== "Any" && !("label" in target && target.label === label)) continue;
        window.__TAURI_INTERNALS__.runCallback(registration.handler, { event, id, payload });
      }
    },
  };
}

beforeEach(() => { clearMocks(); });

describe("project events with multiple open windows", () => {
  it("streams Korean text and archives the answer once through the real IPC client", async () => {
    const transport = projectTransport();
    const store = createPanelStore();
    store.chatSent();
    const client = new IpcClient({
      onMessage(message) {
        if (message.type === "agent_event") store.agentEvent(message.kind, message.detail, message.data);
        if (message.type === "answer") store.answerReceived(message.text);
      },
    });
    await client.connect();
    for (const detail of ["확", "정", "했습니다", ".", " 하", "하"]) {
      for (const label of ["main", "map-agent-project", "map-import-project", "other-project"]) {
        transport.emitTo(label, "agent_event", { sessionId: "session-a", kind: "delta", detail });
      }
    }
    expect(store.getState().turn.answer).toBe("확정했습니다. 하하");
    for (const label of ["main", "map-agent-project", "map-import-project"]) {
      transport.emitTo(label, "answer", { sessionId: "session-a", text: "확정했습니다. 하하" });
    }
    expect(store.getState().log.filter((entry) => entry.kind === "agent").map((entry) => entry.text))
      .toEqual(["확정했습니다. 하하"]);
    client.stop();
  });

  it("delivers project fanout once through Map subscriptions and still receives broadcasts", async () => {
    const transport = projectTransport();
    const subscriptions = createMapEventSubscriptions();
    const received = vi.fn();
    await subscriptions.listen("agent_event", received);
    for (const label of ["main", "map-agent-project", "map-import-project"]) {
      transport.emitTo(label, "agent_event", { detail: "맵" });
    }
    expect(received).toHaveBeenCalledTimes(1);
    transport.emitTo(undefined, "agent_event", { detail: "broadcast" });
    expect(received).toHaveBeenCalledTimes(2);
    subscriptions.dispose();
    transport.emitTo("main", "agent_event", { detail: "stopped" });
    expect(received).toHaveBeenCalledTimes(2);
  });
});
