import { afterEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn(async (..._args: unknown[]) => "tauri"),
  listen: vi.fn(async (..._args: unknown[]) => () => {}),
}));
const remote = vi.hoisted(() => ({
  invoke: vi.fn(async (..._args: unknown[]) => "remote"),
  listen: vi.fn(async (..._args: unknown[]) => () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "main-abc" }) }));
vi.mock("./remoteTransport", () => ({ remoteTransport: () => remote }));

import { invoke, listen, setTransportKind, transportKind } from "./transport";

function at(path: string, tauriRuntime: boolean) {
  window.history.replaceState(null, "", path);
  if (tauriRuntime) {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  } else {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  }
}

afterEach(() => {
  setTransportKind(null);
  at("/", false);
  vi.clearAllMocks();
});

describe("transport selection", () => {
  it("uses Tauri whenever its runtime is present, even under /remote", () => {
    at("/remote/", true);
    expect(transportKind()).toBe("tauri");
  });

  it("uses the remote API on the remote page without Tauri", () => {
    at("/remote/", false);
    expect(transportKind()).toBe("remote");
    at("/remote.html", false);
    expect(transportKind()).toBe("remote");
  });

  it("keeps Tauri elsewhere (desktop pages, tests with mocked Tauri)", () => {
    at("/index.html", false);
    expect(transportKind()).toBe("tauri");
  });

  it("forwards desktop calls to Tauri exactly as given", async () => {
    at("/", true);
    await expect(invoke("status")).resolves.toBe("tauri");
    expect(tauri.invoke).toHaveBeenLastCalledWith("status");
    const bytes = new Uint8Array([1]);
    await invoke("attachment_stage", bytes, { headers: { a: "b" } });
    expect(tauri.invoke).toHaveBeenLastCalledWith("attachment_stage", bytes, { headers: { a: "b" } });
    const handler = () => {};
    await listen("answer", handler);
    expect(tauri.listen).toHaveBeenCalledWith("answer", handler, { target: "main-abc" });
    expect(remote.invoke).not.toHaveBeenCalled();
  });

  it("sends remote-page calls to the remote transport", async () => {
    at("/remote/", false);
    await expect(invoke("session_list")).resolves.toBe("remote");
    expect(remote.invoke).toHaveBeenLastCalledWith("session_list");
    const handler = () => {};
    await listen("answer", handler);
    expect(remote.listen).toHaveBeenCalledWith("answer", handler);
    expect(tauri.invoke).not.toHaveBeenCalled();
    expect(tauri.listen).not.toHaveBeenCalled();
  });
});
