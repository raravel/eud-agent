/**
 * Native-project availability send gate.
 *
 * Setup normally makes the project available. Runtime removal/corruption gates
 * authoring until a successful native status refresh restores availability.
 */
import { describe, it, expect, vi } from "vitest";
import { createPanelStore } from "@/state/store";

function freshStore() {
  return createPanelStore();
}

describe("native project availability", () => {
  it("defaults available after setup", () => {
    expect(freshStore().getState().projectAvailable).toBe(true);
  });

  it("allows send with an available native project", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });

    const state = store.getState();
    expect(state.projectAvailable).toBe(true);
    expect(state.hasProject).toBe(true);
    expect(state.canSend).toBe(true);
  });

  it("blocks send after a native-project unavailable marker", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });

    store.errorReceived("native project unavailable");

    const state = store.getState();
    expect(state.projectAvailable).toBe(false);
    expect(state.canSend).toBe(false);
  });

  it("restores availability when native status succeeds", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: false, project: "P" });
    store.applyList({ files: [] });
    store.errorReceived("native project unavailable");

    store.applyStatus({ compiling: false, project: "P" });

    expect(store.getState().projectAvailable).toBe(true);
    expect(store.getState().canSend).toBe(true);
  });

  it("does not publish an identical periodic status snapshot", () => {
    const store = freshStore();
    const listener = vi.fn();
    const unsubscribe = store.subscribe(listener);

    store.applyStatus({ compiling: false, project: "P" });
    store.applyStatus({ compiling: false, project: "P" });

    expect(listener).toHaveBeenCalledTimes(1);

    store.applyStatus({ compiling: true, project: "P" });
    expect(listener).toHaveBeenCalledTimes(2);
    unsubscribe();
  });

  it("exposes an explicit project availability setter", () => {
    const store = freshStore();

    store.projectAvailabilityChanged(false);
    expect(store.getState().projectAvailable).toBe(false);

    store.projectAvailabilityChanged(true);
    expect(store.getState().projectAvailable).toBe(true);
  });

  it("does not change project availability for unrelated errors", () => {
    const store = freshStore();

    store.projectAvailabilityChanged(false);
    store.errorReceived("some unrelated error");
    expect(store.getState().projectAvailable).toBe(false);

    store.projectAvailabilityChanged(true);
    store.errorReceived("some unrelated error");
    expect(store.getState().projectAvailable).toBe(true);
  });
});
