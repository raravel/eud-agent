import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WorkspaceFileTree } from "./WorkspaceFileTree";
import { openProjectRootIn, type WorkspaceListResponse } from "@/lib/ipc";

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openProjectRootIn: vi.fn().mockResolvedValue(undefined),
}));

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: "plans/req-1.md", size: 16 },
    { path: "specs/combat.md", size: 32 },
    { path: "specs/notes/balance.md", size: 24 },
    { path: "worklog/summary.md", size: 48 },
  ],
};

function callbacks() {
  return {
    onSelect: vi.fn(),
    onRefresh: vi.fn(),
    onSearch: vi.fn().mockResolvedValue([]),
  };
}

function renderTree(
  overrides: Partial<Parameters<typeof WorkspaceFileTree>[0]> = {},
) {
  const handlers = callbacks();
  const view = render(
    <WorkspaceFileTree
      workspace={workspace}
      selectedPath={null}
      loading={false}
      {...handlers}
      {...overrides}
    />,
  );
  return { handlers, view };
}

describe("WorkspaceFileTree", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("renders the project root open with every other folder collapsed", () => {
    const { handlers } = renderTree();

    expect(
      screen.getByRole("navigation", { name: "워크스페이스 파일" }),
    ).toBeInTheDocument();
    // Root open: its direct children are visible.
    expect(screen.getByRole("button", { name: "Example 폴더 접기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "plans 폴더 펼치기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "specs 폴더 펼치기" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "worklog 폴더 펼치기" })).toBeInTheDocument();
    // Nested content stays hidden until expanded.
    expect(screen.queryByText("combat.md")).not.toBeInTheDocument();
    expect(screen.queryByText("summary.md")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "specs 폴더 펼치기" }));
    expect(screen.getByText("combat.md")).toBeInTheDocument();
    expect(screen.queryByText("balance.md")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "notes 폴더 펼치기" }));
    expect(screen.getByText("balance.md")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "worklog 폴더 펼치기" }));
    fireEvent.click(screen.getByRole("button", { name: /summary\.md/ }));
    expect(handlers.onSelect).toHaveBeenCalledWith(workspace.files[3]);
  });

  it("collapses the root and restores persisted expanded folders", () => {
    const firstView = renderTree();
    fireEvent.click(screen.getByRole("button", { name: "specs 폴더 펼치기" }));
    fireEvent.click(screen.getByRole("button", { name: "Example 폴더 접기" }));
    expect(
      screen.queryByRole("button", { name: "plans 폴더 펼치기" }),
    ).not.toBeInTheDocument();
    firstView.view.unmount();

    renderTree();
    expect(
      screen.getByRole("button", { name: "Example 폴더 펼치기" }),
    ).toHaveAttribute("aria-expanded", "false");
    // Reopening the root shows specs still expanded from the previous mount.
    fireEvent.click(screen.getByRole("button", { name: "Example 폴더 펼치기" }));
    expect(
      screen.getByRole("button", { name: "plans 폴더 펼치기" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "specs 폴더 접기" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("combat.md")).toBeInTheDocument();
  });

  it("reveals the active document by expanding its ancestor folders", () => {
    renderTree({ selectedPath: "specs/notes/balance.md" });
    expect(screen.getByText("balance.md")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /balance\.md/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("marks the center document tab's file as current", () => {
    renderTree({ selectedPath: "specs/combat.md" });
    expect(screen.getByRole("button", { name: /combat\.md/ })).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("filters with unified search results showing full relative paths", async () => {
    const { handlers } = renderTree();
    handlers.onSearch.mockResolvedValue(["specs/notes/balance.md"]);

    fireEvent.change(
      screen.getByRole("searchbox", { name: "파일명 또는 내용 검색" }),
      { target: { value: "balance" } },
    );

    await waitFor(() => {
      expect(handlers.onSearch).toHaveBeenCalledWith("balance");
      expect(screen.getByText("specs/notes/balance.md")).toBeInTheDocument();
    });
    expect(screen.queryByText("combat.md")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "검색어 지우기" }));
    expect(screen.queryByText("specs/notes/balance.md")).not.toBeInTheDocument();
  });

  it("refreshes the workspace on demand", () => {
    const { handlers } = renderTree();
    fireEvent.click(
      screen.getByRole("button", { name: "워크스페이스 새로 고침" }),
    );
    expect(handlers.onRefresh).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["VSCode로 열기", "vscode"],
    ["파일 탐색기로 열기", "fileManager"],
  ] as const)("opens the project root from the menu: %s", async (label, target) => {
    vi.mocked(openProjectRootIn).mockClear();
    renderTree();
    const trigger = screen.getByRole("button", { name: "프로젝트 폴더 열기 메뉴" });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" });
    fireEvent.click(await screen.findByRole("menuitem", { name: label }));
    expect(openProjectRootIn).toHaveBeenCalledExactlyOnceWith(target);
  });
});
