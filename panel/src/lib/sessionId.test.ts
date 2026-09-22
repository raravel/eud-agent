import { describe, expect, it } from "vitest";

import { shortSessionId } from "./sessionId";

describe("shortSessionId", () => {
  it("keeps the leading eight characters of a UUID", () => {
    expect(shortSessionId("a3f9c2d1-7b1e-4c2a-9d0f-1234567890ab")).toBe("a3f9c2d1");
  });

  it("keeps a shorter id whole", () => {
    expect(shortSessionId("eps-1")).toBe("eps-1");
    expect(shortSessionId("")).toBe("");
  });
});
