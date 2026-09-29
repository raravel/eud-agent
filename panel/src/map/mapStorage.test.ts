import { afterEach, describe, expect, it, vi } from "vitest";
import {
  isOtherProjectEvent,
  projectStorageKey,
  readProjectState,
  writeProjectState,
} from "./mapStorage";

describe("per-project Map window storage", () => {
  afterEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it("copies the single-project value once per project and keeps it", () => {
    localStorage.setItem("map-agent.layout/1", '{"leftWidth":300}');

    expect(readProjectState("map-agent.layout", "aaaaaaaaaaaa")).toBe('{"leftWidth":300}');
    writeProjectState("map-agent.layout", "aaaaaaaaaaaa", '{"leftWidth":420}');
    expect(readProjectState("map-agent.layout", "aaaaaaaaaaaa")).toBe('{"leftWidth":420}');
    // Another project starts from the old value, never from A's.
    expect(readProjectState("map-agent.layout", "bbbbbbbbbbbb")).toBe('{"leftWidth":300}');
    expect(localStorage.getItem("map-agent.layout/1")).toBe('{"leftWidth":300}');
    expect(localStorage.getItem(projectStorageKey("map-agent.layout", "aaaaaaaaaaaa"))).toBe(
      '{"leftWidth":420}',
    );
  });

  it("stores nothing and throws nothing when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readProjectState("map-agent.surface", "aaaaaaaaaaaa")).toBeNull();
    expect(() => writeProjectState("map-agent.surface", "aaaaaaaaaaaa", "{}")).not.toThrow();
  });

  it("recognizes an event of another project", () => {
    expect(isOtherProjectEvent({ projectId: "b" }, "a")).toBe(true);
    expect(isOtherProjectEvent({ projectId: "a" }, "a")).toBe(false);
    expect(isOtherProjectEvent({ sessionId: "s" }, "a")).toBe(false);
    expect(isOtherProjectEvent({ projectId: "b" }, undefined)).toBe(false);
    expect(isOtherProjectEvent(null, "a")).toBe(false);
  });
});
