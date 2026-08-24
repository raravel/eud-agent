import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

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
    onSearch: vi.fn().mockResolvedValue([]),
  };
}

describe("WorkspaceView", () => {
  beforeEach(() => {
    localStorage.clear();
  });

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

  it("collapses top-level folders and restores their persisted state", () => {
    const handlers = callbacks();
    const view = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath={null}
        selectedContent={null}
        loading={false}
        error={null}
        {...handlers}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "specs 폴더 접기" }));
    expect(screen.queryByText("combat.md")).not.toBeInTheDocument();
    view.unmount();

    render(
      <WorkspaceView
        workspace={workspace}
        selectedPath={null}
        selectedContent={null}
        loading={false}
        error={null}
        {...handlers}
      />,
    );
    expect(
      screen.getByRole("button", { name: "specs 폴더 펼치기" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("combat.md")).not.toBeInTheDocument();
  });

  it("filters the tree with unified filename and content search results", async () => {
    const handlers = callbacks();
    handlers.onSearch.mockResolvedValue(["specs/combat.md"]);
    render(
      <WorkspaceView
        workspace={workspace}
        selectedPath={null}
        selectedContent={null}
        loading={false}
        error={null}
        {...handlers}
      />,
    );

    fireEvent.change(
      screen.getByRole("searchbox", { name: "파일명 또는 내용 검색" }),
      { target: { value: "confirmed behavior" } },
    );

    await waitFor(() => {
      expect(handlers.onSearch).toHaveBeenCalledWith("confirmed behavior");
      expect(screen.queryByText("main.eps")).not.toBeInTheDocument();
    });
    expect(screen.getByText("combat.md")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "검색어 지우기" }));
    expect(screen.getByText("main.eps")).toBeInTheDocument();
  });

  it("resizes the file tree vertically and restores the persisted height", () => {
    const firstView = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent="# Combat specification"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    const splitter = screen.getByRole("separator", {
      name: "파일 트리와 문서 높이 조절",
    });
    vi.spyOn(splitter.parentElement!.parentElement!, "getBoundingClientRect").mockReturnValue({
      bottom: 600,
      height: 600,
      left: 0,
      right: 344,
      top: 0,
      width: 344,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });

    fireEvent.pointerDown(splitter, { pointerId: 1, clientY: 192 });
    fireEvent.pointerMove(splitter, { pointerId: 1, clientY: 292 });
    fireEvent.pointerUp(splitter, { pointerId: 1, clientY: 292 });

    expect(
      screen.getByRole("navigation", { name: "워크스페이스 파일" }),
    ).toHaveStyle({ height: "292px" });
    expect(localStorage.getItem("eud.workspace.split")).toBe(
      '{"treeHeight":292,"collapsed":null}',
    );

    firstView.unmount();
    render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent="# Combat specification"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    expect(
      screen.getByRole("navigation", { name: "워크스페이스 파일" }),
    ).toHaveStyle({ height: "292px" });
  });

  it("collapses and restores either side of the workspace splitter", () => {
    const view = render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent="# Combat specification"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    const fileTree = screen.getByRole("navigation", {
      name: "워크스페이스 파일",
    });
    const preview = screen.getByRole("article");

    fireEvent.click(screen.getByRole("button", { name: "파일 트리 접기" }));
    expect(fileTree).not.toBeVisible();
    expect(preview).toBeVisible();
    expect(
      screen.getByRole("button", { name: "파일 트리 펼치기" }),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "파일 트리 펼치기" }));
    fireEvent.click(screen.getByRole("button", { name: "문서 미리보기 접기" }));
    expect(fileTree).toBeVisible();
    expect(preview).not.toBeVisible();
    expect(localStorage.getItem("eud.workspace.split")).toBe(
      '{"treeHeight":192,"collapsed":"preview"}',
    );

    view.unmount();
    render(
      <WorkspaceView
        workspace={workspace}
        selectedPath="specs/combat.md"
        selectedContent="# Combat specification"
        loading={false}
        error={null}
        {...callbacks()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "문서 미리보기 펼치기" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("article", { hidden: true })).not.toBeVisible();
  });
});
