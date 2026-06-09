import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MemoryView } from "./MemoryView";
import type { MemoryViewState } from "../state/store";
import type { MemoryFile } from "../lib/protocol";

vi.mock("@/components/MonacoEditor", async () => {
  const React = await import("react");

  function MonacoEditor({
    value = "",
    onChange,
    language,
  }: {
    value?: string;
    onChange?: (value: string) => void;
    language?: string;
  }) {
    return React.createElement("textarea", {
      "aria-label": "메모리 편집기",
      "data-language": language,
      value,
      onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) =>
        onChange?.(event.currentTarget.value),
    });
  }

  return {
    default: MonacoEditor,
    MonacoEditor,
  };
});

const files: Record<MemoryFile, string> = {
  resources: "# Resources\n",
  structure: "# Structure\n",
  conventions: "# Conventions\n",
  lessons: "# Lessons\n",
};

const cleanDirty: Record<MemoryFile, boolean> = {
  resources: false,
  structure: false,
  conventions: false,
  lessons: false,
};

function memoryState(overrides: Partial<MemoryViewState> = {}): MemoryViewState {
  return {
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
        ts: "2026-06-09T09:30:00Z",
        request_id: "req-1",
        instruction: "프로젝트 메모리 확인",
        kind: "analysis",
        tools: [],
        files: [],
        decision: "읽기 전용 에피소드 목록을 표시한다.",
      },
    ],
    activeTab: "resources",
    drafts: {},
    dirty: cleanDirty,
    ...overrides,
  };
}

function callbacks() {
  return {
    onClose: vi.fn(),
    onTabSelected: vi.fn(),
    onEdited: vi.fn(),
    onSave: vi.fn(),
  };
}

function renderMemoryView(memory: MemoryViewState = memoryState(), handlers = callbacks()) {
  return {
    handlers,
    ...render(
      <MemoryView
        memory={memory}
        onClose={handlers.onClose}
        onTabSelected={handlers.onTabSelected}
        onEdited={handlers.onEdited}
        onSave={handlers.onSave}
      />,
    ),
  };
}

describe("MemoryView", () => {
  it("renders the four memory tabs and a markdown editor for the active tab", async () => {
    renderMemoryView();

    expect(screen.getByRole("tab", { name: "리소스" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "구조" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "컨벤션" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "교훈" })).toBeInTheDocument();

    const editor = await screen.findByLabelText("메모리 편집기");
    expect(editor).toHaveValue(files.resources);
    expect(editor).toHaveAttribute("data-language", "markdown");
  });

  it("editing a tab records the draft, enables Save when dirty, and saves {file, content}", async () => {
    const handlers = callbacks();
    const { rerender } = renderMemoryView(memoryState(), handlers);
    const updated = "# Resources\nUpdated local context.\n";

    expect(screen.getByRole("button", { name: /저장/ })).toBeDisabled();
    fireEvent.change(await screen.findByLabelText("메모리 편집기"), {
      target: { value: updated },
    });

    expect(handlers.onEdited).toHaveBeenCalledWith("resources", updated);

    rerender(
      <MemoryView
        memory={memoryState({
          drafts: { resources: updated },
          dirty: { ...cleanDirty, resources: true },
        })}
        onClose={handlers.onClose}
        onTabSelected={handlers.onTabSelected}
        onEdited={handlers.onEdited}
        onSave={handlers.onSave}
      />,
    );

    const saveButton = screen.getByRole("button", { name: /저장/ });
    expect(saveButton).toBeEnabled();
    fireEvent.click(saveButton);

    expect(handlers.onSave).toHaveBeenCalledWith({
      file: "resources",
      content: updated,
    });
  });

  it("selects tabs by memory file id and shows a draft for the active tab", async () => {
    const handlers = callbacks();
    renderMemoryView(
      memoryState({
        activeTab: "conventions",
        drafts: { conventions: "# Conventions\nUse Korean labels.\n" },
        dirty: { ...cleanDirty, conventions: true },
      }),
      handlers,
    );

    expect(await screen.findByLabelText("메모리 편집기")).toHaveValue(
      "# Conventions\nUse Korean labels.\n",
    );

    fireEvent.click(screen.getByRole("tab", { name: "교훈" }));
    expect(handlers.onTabSelected).toHaveBeenCalledWith("lessons");
  });

  it("renders episodes newest first as read-only history", () => {
    renderMemoryView();

    const newest = screen.getByText("패널 메모리 보기 구현");
    const older = screen.getByText("프로젝트 메모리 확인");
    expect(newest.compareDocumentPosition(older) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText(/req-2/)).toBeInTheDocument();
    expect(screen.getByText("Monaco markdown editor를 사용한다.")).toBeInTheDocument();
    expect(screen.getByText("apply_patch")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("패널 메모리 보기 구현")).not.toBeInTheDocument();
  });

  it("closes when the close button is clicked", () => {
    const handlers = callbacks();
    renderMemoryView(memoryState(), handlers);

    fireEvent.click(screen.getByRole("button", { name: "닫기" }));

    expect(handlers.onClose).toHaveBeenCalledTimes(1);
  });
});
