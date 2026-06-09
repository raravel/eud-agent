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
      episodes: [
        {
          ts: "2026-06-09T10:00:00Z",
          request_id: "req-2",
          instruction: "패널 메모리 보기 구현",
          kind: "implementation",
          tools: ["apply_patch"],
          files: ["panel/src/components/MemoryView.tsx"],
          decision: "Monaco markdown editor를 사용한다.",
        },
        {
          instruction: "필드가 일부 누락된 에피소드도 안전하게 표시한다.",
        },
      ],
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
        episodes: [],
      }),
    ).toBe(false);
    expect(
      isMemoryMessage({
        type: "memory",
        project: "eud-agent",
        files,
        episodes: "not-an-array",
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
