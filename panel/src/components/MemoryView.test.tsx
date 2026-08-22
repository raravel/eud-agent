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
    height,
  }: {
    value?: string;
    onChange?: (value: string) => void;
    language?: string;
    height?: string | number;
  }) {
    return React.createElement("textarea", {
      "aria-label": "메모리 편집기",
      "data-language": language,
      "data-height": height,
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
    expect(screen.queryByText("에피소드")).not.toBeInTheDocument();

    const editor = await screen.findByLabelText("메모리 편집기");
    expect(editor).toHaveValue(files.resources);
    expect(editor).toHaveAttribute("data-language", "markdown");
  });

  it("fills the available project-sidebar height with the editor", async () => {
    const handlers = callbacks();
    render(
      <MemoryView
        memory={memoryState()}
        embedded
        onClose={handlers.onClose}
        onTabSelected={handlers.onTabSelected}
        onEdited={handlers.onEdited}
        onSave={handlers.onSave}
      />,
    );

    expect(await screen.findByLabelText("메모리 편집기")).toHaveAttribute(
      "data-height",
      "100%",
    );
    expect(screen.queryByRole("button", { name: "닫기" })).not.toBeInTheDocument();
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


  it("closes when the close button is clicked", () => {
    const handlers = callbacks();
    renderMemoryView(memoryState(), handlers);

    fireEvent.click(screen.getByRole("button", { name: "닫기" }));

    expect(handlers.onClose).toHaveBeenCalledTimes(1);
  });
});
