import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { createMapEventSubscriptions } from "./mapEventSubscriptions";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "map-agent-project" }) }));

function deferredRegistration() {
  let complete: (unlisten: UnlistenFn) => void = () => {
    throw new Error("registration not started");
  };
  const promise = new Promise<UnlistenFn>((resolve) => { complete = resolve; });
  return { promise, complete };
}

beforeEach(() => { vi.mocked(listen).mockReset(); });

describe("Map turn event subscriptions", () => {
  it("removes a listener whose registration completes after effect cleanup", async () => {
    const registration = deferredRegistration();
    const unlisten = vi.fn();
    vi.mocked(listen).mockReturnValueOnce(registration.promise);
    const scope = createMapEventSubscriptions();
    const pending = scope.listen("agent_event", vi.fn());
    scope.dispose();
    registration.complete(unlisten);
    await pending;
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("stops a disposed sequential registration from subscribing to further events", async () => {
    const scope = createMapEventSubscriptions();
    scope.dispose();
    await scope.listen("agent_event", vi.fn());
    expect(listen).not.toHaveBeenCalled();
  });

  it("appends each Korean delta once after the old effect is replaced", async () => {
    const callbacks: Array<() => void> = [];
    const unlisten = vi.fn();
    vi.mocked(listen).mockImplementation(async (name, handler) => {
      callbacks.push(() => handler({ event: name, id: 1, payload: undefined }));
      return unlisten;
    });
    let text = "";
    const append = () => { text += "현재 "; };
    const previous = createMapEventSubscriptions();
    await previous.listen("agent_event", append);
    previous.dispose();
    const current = createMapEventSubscriptions();
    await current.listen("agent_event", append);
    // A native event already queued before unlisten can still reach its callback.
    for (const callback of callbacks) callback();
    expect(text).toBe("현재 ");
    current.dispose();
  });

  it("cleans up active listeners only once", async () => {
    const unlisten = vi.fn();
    vi.mocked(listen).mockResolvedValue(unlisten);
    const scope = createMapEventSubscriptions();
    await scope.listen("agent_event", vi.fn());
    scope.dispose();
    scope.dispose();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});

it("preserves intentional repeated deltas from the active subscription", async () => {
  const callbacks: Array<() => void> = [];
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    callbacks.push(() => handler({ event: name, id: 1, payload: undefined }));
    return vi.fn();
  });
  const scope = createMapEventSubscriptions();
  let text = "";
  await scope.listen("agent_event", () => { text += "하"; });
  for (const callback of callbacks) { callback(); callback(); }
  expect(text).toBe("하하");
  scope.dispose();
});
