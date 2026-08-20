import { describe, expect, it } from "vitest";

import { isMemoryMessage, isMemorySavedMessage, isServerMessage } from "./protocol";

const files = {
  resources: "# Resources\n",
  structure: "# Structure\n",
  conventions: "# Conventions\n",
  lessons: "# Lessons\n",
};

describe("memory protocol guards", () => {
  it("accepts a valid memory message and routes it through isServerMessage", () => {
    const message = {
      type: "memory",
      project: "eud-agent",
      files,
    };

    expect(isMemoryMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
  });

  it("rejects malformed memory messages", () => {
    expect(isMemoryMessage({ type: "memory" })).toBe(false);
    expect(
      isMemoryMessage({
        type: "memory",
        project: "eud-agent",
        files: {
          resources: "# Resources\n",
          structure: "# Structure\n",
          conventions: "# Conventions\n",
        },
      }),
    ).toBe(false);
    expect(
      isMemoryMessage({
        type: "memory",
        project: "eud-agent",
        files: { ...files, lessons: 42 },
      }),
    ).toBe(false);
    expect(isMemoryMessage({ type: "memory_saved", file: "resources" })).toBe(false);
  });

  it("accepts a valid memory_saved message and routes it through isServerMessage", () => {
    const message = {
      type: "memory_saved",
      file: "lessons",
    };

    expect(isMemorySavedMessage(message)).toBe(true);
    expect(isServerMessage(message)).toBe(true);
  });

  it("rejects malformed memory_saved messages", () => {
    expect(isMemorySavedMessage({ type: "memory_saved" })).toBe(false);
    expect(isMemorySavedMessage({ type: "memory_saved", file: "notes" })).toBe(false);
    expect(isMemorySavedMessage({ type: "memory", file: "resources" })).toBe(false);
  });
});
