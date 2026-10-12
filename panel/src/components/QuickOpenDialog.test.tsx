import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { QuickOpenDialog } from "./QuickOpenDialog";
import type { WorkspaceListResponse } from "@/lib/ipc";

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: ".git/config", size: 4 },
    { path: "project.eap", size: 8 },
    { path: "src/main.eps", size: 16 },
    { path: "src/combat/main_loop.eps", size: 24 },
    { path: "src/domain.eps", size: 12 },
    { path: ".eud-agent/workspace/specs/main.md", size: 32 },
  ],
};

function renderDialog() {
  const onOpenFile = vi.fn();
  const onOpenChange = vi.fn();
  render(
    <QuickOpenDialog
      open
      onOpenChange={onOpenChange}
      workspace={workspace}
      onOpenFile={onOpenFile}
    />,
  );
  return { onOpenFile, onOpenChange, input: screen.getByPlaceholderText("파일 이름으로 검색") };
}

function optionNames() {
  return within(screen.getByRole("listbox"))
    .getAllByRole("option")
    .map((option) => option.getAttribute("title"));
}

describe("QuickOpenDialog", () => {
  it("ranks file names over paths and never lists .git entries", () => {
    const { input } = renderDialog();
    expect(optionNames()).not.toContain(".git/config");

    fireEvent.change(input, { target: { value: "main" } });
    expect(optionNames()).toEqual([
      "src/main.eps",
      "src/combat/main_loop.eps",
      "src/domain.eps",
      ".eud-agent/workspace/specs/main.md",
    ]);
    // The matched part of the name is marked.
    const first = screen.getAllByRole("option")[0];
    expect(within(first).getByText("main", { selector: "mark" })).toBeInTheDocument();
    expect(within(first).getByText("src")).toBeInTheDocument();

    fireEvent.change(input, { target: { value: "config" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("일치하는 파일이 없습니다.")).toBeInTheDocument();
  });

  it("opens the highlighted file with Enter after arrow navigation and closes", () => {
    const { input, onOpenFile, onOpenChange } = renderDialog();
    fireEvent.change(input, { target: { value: "main" } });
    expect(screen.getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onOpenFile).toHaveBeenCalledExactlyOnceWith(workspace.files[3]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("opens a clicked file", () => {
    const { input, onOpenFile } = renderDialog();
    fireEvent.change(input, { target: { value: "domain" } });
    fireEvent.click(screen.getByTitle("src/domain.eps"));
    expect(onOpenFile).toHaveBeenCalledExactlyOnceWith(workspace.files[4]);
  });
});
