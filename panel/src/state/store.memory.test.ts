import { describe, expect, it } from "vitest";

import { createPanelStore } from "./store";
import type { Episode, MemoryFile } from "../lib/protocol";

const files: Record<MemoryFile, string> = {
  resources: "# Resources\n",
  structure: "# Structure\n",
  conventions: "# Conventions\n",
  lessons: "# Lessons\n",
};

const episodes: Episode[] = [
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
    ts: "2026-06-09T09:30:00Z",
    request_id: "req-1",
    instruction: "프로젝트 메모리 확인",
    kind: "analysis",
    tools: [],
    files: [],
    decision: "읽기 전용 에피소드 목록을 표시한다.",
  },
];

function dirtyFlags(value = false): Record<MemoryFile, boolean> {
  return {
    resources: value,
    structure: value,
    conventions: value,
    lessons: value,
  };
}

describe("panel store memory reducers", () => {
  it("memoryReceived populates files and episodes while clearing drafts and dirty flags", () => {
    const store = createPanelStore();

    store.memoryReceived("eud-agent", files, episodes);

    expect(store.getState().memory).toEqual({
      project: "eud-agent",
      files,
      episodes,
      activeTab: "resources",
      drafts: {},
      dirty: dirtyFlags(false),
    });
  });

  it("memoryEdited marks a tab dirty only when the draft differs from persisted content", () => {
    const store = createPanelStore();
    store.memoryReceived("eud-agent", files, episodes);

    store.memoryEdited("resources", "# Resources\nUpdated\n");
    expect(store.getState().memory?.drafts.resources).toBe("# Resources\nUpdated\n");
    expect(store.getState().memory?.dirty.resources).toBe(true);

    store.memoryEdited("structure", files.structure);
    expect(store.getState().memory?.drafts.structure).toBe(files.structure);
    expect(store.getState().memory?.dirty.structure).toBe(false);
  });

  it("memorySaved clears the saved tab dirty flag and commits the current draft", () => {
    const store = createPanelStore();
    store.memoryReceived("eud-agent", files, episodes);

    store.memoryEdited("lessons", "# Lessons\nUse local Monaco wiring.\n");
    store.memorySaved("lessons");

    const memory = store.getState().memory;
    expect(memory?.files.lessons).toBe("# Lessons\nUse local Monaco wiring.\n");
    expect(memory?.drafts.lessons).toBeUndefined();
    expect(memory?.dirty.lessons).toBe(false);
  });

  it("memoryOpened and memoryClosed toggle memoryOpen", () => {
    const store = createPanelStore();

    expect(store.getState().memoryOpen).toBe(false);
    store.memoryOpened();
    expect(store.getState().memoryOpen).toBe(true);
    store.memoryClosed();
    expect(store.getState().memoryOpen).toBe(false);
  });

  it("memoryTabSelected updates the active tab without changing file contents", () => {
    const store = createPanelStore();
    store.memoryReceived("eud-agent", files, episodes);

    store.memoryTabSelected("conventions");

    expect(store.getState().memory?.activeTab).toBe("conventions");
    expect(store.getState().memory?.files).toEqual(files);
  });

  it("memorySaveSent does not change phase, canSend, or dirty state while awaiting memory_saved", () => {
    const store = createPanelStore();
    store.memoryReceived("eud-agent", files, episodes);
    store.memoryEdited("resources", "# Resources\nUpdated\n");
    const before = store.getState();

    store.memorySaveSent("resources");

    const after = store.getState();
    expect(after.phase).toBe(before.phase);
    expect(after.canSend).toBe(before.canSend);
    expect(after.memory?.dirty.resources).toBe(true);
  });

  it("opening memory is an orthogonal overlay and does not change phase or canSend", () => {
    const store = createPanelStore();
    const before = store.getState();

    store.memoryOpened();

    const after = store.getState();
    expect(after.phase).toBe(before.phase);
    expect(after.canSend).toBe(before.canSend);
    expect(after.memoryOpen).toBe(true);
  });
});
