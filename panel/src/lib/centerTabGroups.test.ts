import { describe, expect, it } from "vitest";

import {
  centerTabForKey,
  emptyCenterTabGroup,
  parseCenterTabGroups,
  pruneCenterTabGroups,
} from "./centerTabGroups";

const key = (init: Partial<KeyboardEvent>) => ({
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  code: "",
  key: "",
  ...init,
});

describe("centerTabForKey", () => {
  const tabs = ["chat", "src/a.eps", "src/b.eps", "virtual:sounds"];

  it("Alt+digit picks that position and Alt+9 the last tab", () => {
    expect(centerTabForKey(key({ altKey: true, code: "Digit1" }), tabs, "src/b.eps")).toBe("chat");
    expect(centerTabForKey(key({ altKey: true, code: "Digit3" }), tabs, "chat")).toBe("src/b.eps");
    expect(centerTabForKey(key({ altKey: true, code: "Digit9" }), tabs, "chat")).toBe("virtual:sounds");
    expect(centerTabForKey(key({ altKey: true, code: "Digit7" }), tabs, "chat")).toBeNull();
  });

  it("Ctrl+PageUp/PageDown step and wrap around", () => {
    expect(centerTabForKey(key({ ctrlKey: true, key: "PageDown" }), tabs, "chat")).toBe("src/a.eps");
    expect(centerTabForKey(key({ ctrlKey: true, key: "PageDown" }), tabs, "virtual:sounds")).toBe("chat");
    expect(centerTabForKey(key({ ctrlKey: true, key: "PageUp" }), tabs, "chat")).toBe("virtual:sounds");
  });

  it("ignores other keys and modifier mixes", () => {
    expect(centerTabForKey(key({ altKey: true, code: "ArrowLeft" }), tabs, "chat")).toBeNull();
    expect(centerTabForKey(key({ altKey: true, shiftKey: true, code: "Digit1" }), tabs, "chat")).toBeNull();
    expect(centerTabForKey(key({ ctrlKey: true, altKey: true, key: "PageUp" }), tabs, "chat")).toBeNull();
  });
});

describe("center tab group storage", () => {
  it("parses stored groups and drops malformed entries", () => {
    const groups = parseCenterTabGroups(
      JSON.stringify({
        s1: {
          documents: ["src/a.eps", 3],
          references: [{ id: "reference:x", hitId: "h", title: "T" }, { id: 1 }],
          datWiki: true,
          sounds: "yes",
          active: "src/a.eps",
        },
        s2: "broken",
      }),
    );
    expect(groups).toEqual({
      s1: {
        documents: ["src/a.eps"],
        references: [{ id: "reference:x", hitId: "h", title: "T" }],
        datWiki: true,
        sounds: false,
        active: "src/a.eps",
      },
    });
    expect(parseCenterTabGroups("not json")).toEqual({});
  });

  it("prunes deleted sessions and empty groups", () => {
    const kept = { ...emptyCenterTabGroup(), documents: ["src/a.eps"] };
    const pruned = pruneCenterTabGroups(
      { live: kept, gone: kept, empty: emptyCenterTabGroup() },
      new Set(["live", "empty"]),
    );
    expect(pruned).toEqual({ live: kept });
  });
});
