import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { WorkspaceView } from "./WorkspaceView";
import type { WorkspaceListResponse } from "@/lib/ipc";

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: "specs/combat.md", source: false, size: 32 },
    { path: "source/main.eps", source: true, size: 48 },
  ],
};

function callbacks() {
  return {
    onSelect: vi.fn(),
    onRefresh: vi.fn(),
    onClose: vi.fn(),
  };
}

describe("WorkspaceView", () => {
  it("renders the document tree and a Markdown preview", () => {
    const handlers = callbacks();
    const { container } = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent="# Combat specification\n\nConfirmed behavior."
        loading={false}
        error={null}
        {...handlers}
      />,
    );

    expect(screen.getByRole("navigation", { name: "워크스페이스 파일" })).toBeInTheDocument();
    expect(screen.getByText("combat.md")).toBeInTheDocument();
    expect(screen.getByText("main.eps")).toBeInTheDocument();
    expect(container).toHaveTextContent("Combat specification");
    expect(screen.getByText("검토 대상 문서")).toBeInTheDocument();
  });

  it("omits the duplicate close button when embedded in the project sidebar", () => {
    render(
      <WorkspaceView
        workspace={workspace}
        selectedPath={null}
        selectedContent={null}
        loading={false}
        error={null}
        embedded
        {...callbacks()}
      />,
    );

    expect(
      screen.queryByRole("button", { name: "워크스페이스 닫기" }),
    ).not.toBeInTheDocument();
  });

  it("navigates relative Markdown links inside the workspace wiki", async () => {
    const handlers = callbacks();
    const wikiWorkspace: WorkspaceListResponse = {
      ...workspace,
      files: [
        { path: "specs/index.md", source: false, size: 48 },
        ...workspace.files,
      ],
    };
    render(
      <WorkspaceView
        workspace={wikiWorkspace}
        selectedPath="specs/index.md"
        selectedContent={"# Project wiki\n\nOpen [Combat](combat.md).\n"}
        loading={false}
        error={null}
        {...handlers}
      />,
    );

    fireEvent.click(await screen.findByRole("link", { name: "Combat" }));
    expect(handlers.onSelect).toHaveBeenCalledWith(wikiWorkspace.files[1]);
  });

  it("selects a source file and exposes its read-only state", () => {
    const handlers = callbacks();
    const { rerender } = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath={null}
        selectedContent={null}
        loading={false}
        error={null}
        {...handlers}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /main\.eps/ }));
    expect(handlers.onSelect).toHaveBeenCalledWith(workspace.files[1]);

    rerender(
      <WorkspaceView
        workspace={workspace}
        selectedPath="source/main.eps"
        selectedContent="function onPluginStart() {}"
        loading={false}
        error={null}
        {...handlers}
      />,
    );
    expect(screen.getByText("읽기 전용 소스")).toBeInTheDocument();
    expect(screen.getByText(/function onPluginStart/)).toBeInTheDocument();
  });

  it("renders parent-owned acceptance metadata separately from document text", () => {
    render(
      <WorkspaceView
        workspace={{
          ...workspace,
          files: [
            {
              ...workspace.files[0],
              state: "accepted",
              revision: 3,
            },
          ],
        }}
        selectedPath="specs/combat.md"
        selectedContent="# Accepted"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    expect(screen.getByText("확정됨 · r3")).toBeInTheDocument();
  });

  it("renders authoritative approval metadata for saved plans", () => {
    render(
      <WorkspaceView
        workspace={{
          ...workspace,
          files: [
            {
              path: "plans/req-1.md",
              source: false,
              size: 24,
              state: "approved",
              revision: 2,
            },
          ],
        }}
        selectedPath="plans/req-1.md"
        selectedContent="# Approved plan"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    expect(screen.getByText("승인된 계획 · r2")).toBeInTheDocument();
  });

  it("reports loading and read failures accessibly", () => {
    const handlers = callbacks();
    const { rerender } = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent={null}
        loading
        error={null}
        {...handlers}
      />,
    );
    expect(screen.getByText("파일을 여는 중…")).toBeInTheDocument();

    rerender(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent={null}
        loading={false}
        error="읽기 실패"
        {...handlers}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("읽기 실패");
  });
});
