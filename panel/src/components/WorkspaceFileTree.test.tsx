import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WorkspaceFileTree } from "./WorkspaceFileTree";
import {
  openProjectRootIn,
  type WorkspaceListResponse,
  type WorkspaceSearchResponse,
} from "@/lib/ipc";

vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  openProjectRootIn: vi.fn().mockResolvedValue(undefined),
}));

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: "project.eap", size: 8 },
    { path: "plans/req-1.md", size: 16 },
    { path: "specs/combat.md", size: 32 },
    { path: "specs/notes/balance.md", size: 24 },
    { path: "worklog/summary.md", size: 48 },
  ],
};

function searchResponse(
  query: string,
  files: WorkspaceSearchResponse["files"],
  truncated = false,
): WorkspaceSearchResponse {
  return { workspaceId: workspace.workspaceId, query, files, truncated };
}

function callbacks() {
  return {
    onSelect: vi.fn(),
    onRefresh: vi.fn(),
    onSearch: vi.fn().mockResolvedValue(searchResponse("", [])),
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

  it("lists the project root's entries at the top level with folders collapsed", () => {
    const { handlers } = renderTree();

    expect(
      screen.getByRole("navigation", { name: "워크스페이스 파일" }),
    ).toBeInTheDocument();
    // No row for the root folder itself: its entries are the top level.
    expect(screen.queryByRole("button", { name: /^Example 폴더/ })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /project\.eap/ })).toBeInTheDocument();
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
    expect(handlers.onSelect).toHaveBeenCalledWith(workspace.files[4]);
  });

  it("restores persisted expanded folders", () => {
    const firstView = renderTree();
    fireEvent.click(screen.getByRole("button", { name: "specs 폴더 펼치기" }));
    firstView.view.unmount();

    renderTree();
    expect(
      screen.getByRole("button", { name: "specs 폴더 접기" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("combat.md")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "plans 폴더 펼치기" }),
    ).toHaveAttribute("aria-expanded", "false");
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

  it("replaces the tree with content matches grouped per file", async () => {
    const { handlers } = renderTree();
    handlers.onSearch.mockResolvedValue(
      searchResponse("밸런스", [
        {
          path: "specs/combat.md",
          matches: [{ line: 4, preview: "전투 밸런스 조정", ranges: [[3, 6]] }],
        },
        {
          path: "specs/notes/balance.md",
          matches: [
            { line: 1, preview: "# 밸런스", ranges: [[2, 5]] },
            { line: 12, preview: "밸런스 표와 밸런스 메모", ranges: [[0, 3], [7, 10]] },
          ],
        },
      ]),
    );

    fireEvent.change(screen.getByRole("searchbox", { name: "내용 검색" }), {
      target: { value: "밸런스" },
    });

    const results = await screen.findByRole("list", { name: "검색 결과" });
    expect(handlers.onSearch).toHaveBeenCalledExactlyOnceWith("밸런스");
    expect(screen.getByRole("status")).toHaveTextContent("2개 파일에서 3개 결과");
    expect(screen.queryByRole("list", { name: "프로젝트 파일 트리" })).not.toBeInTheDocument();

    const header = within(results).getByRole("button", {
      name: "specs/notes/balance.md 결과 2개 접기",
    });
    expect(header).toHaveAttribute("aria-expanded", "true");
    expect(header).toHaveTextContent("balance.md");
    expect(header).toHaveTextContent("specs/notes");
    const matches = screen.getByRole("list", { name: "specs/notes/balance.md 일치 항목" });
    const rows = within(matches).getAllByRole("button");
    expect(rows).toHaveLength(2);
    expect(rows[1]).toHaveTextContent("12");
    expect(within(rows[1]).getAllByText("밸런스", { selector: "mark" })).toHaveLength(2);

    // A match opens its file at that line.
    fireEvent.click(rows[1]);
    expect(handlers.onSelect).toHaveBeenCalledExactlyOnceWith(workspace.files[3], { line: 12 });

    // The header collapses its matches without opening anything.
    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(
      screen.queryByRole("list", { name: "specs/notes/balance.md 일치 항목" }),
    ).not.toBeInTheDocument();
    expect(handlers.onSelect).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "검색어 지우기" }));
    expect(screen.queryByRole("list", { name: "검색 결과" })).not.toBeInTheDocument();
    expect(screen.getByRole("list", { name: "프로젝트 파일 트리" })).toBeInTheDocument();
  });

  it("reports truncated, empty, and failed searches", async () => {
    const { handlers } = renderTree();
    const search = screen.getByRole("searchbox", { name: "내용 검색" });
    handlers.onSearch.mockResolvedValue(
      searchResponse(
        "x",
        [{ path: "project.eap", matches: [{ line: 1, preview: "x", ranges: [[0, 1]] }] }],
        true,
      ),
    );
    fireEvent.change(search, { target: { value: "x" } });
    expect(await screen.findByText(/결과가 많아 일부만 표시합니다/)).toBeInTheDocument();

    handlers.onSearch.mockResolvedValue(searchResponse("none", []));
    fireEvent.change(search, { target: { value: "none" } });
    expect(await screen.findByText("검색 결과가 없습니다.")).toBeInTheDocument();

    handlers.onSearch.mockRejectedValue(new Error("boom"));
    fireEvent.change(search, { target: { value: "fail" } });
    expect(await screen.findByRole("alert")).toHaveTextContent("검색하지 못했습니다");
  });

  it("keeps a finished search when the file list refreshes", async () => {
    const { handlers, view } = renderTree();
    handlers.onSearch.mockResolvedValue(
      searchResponse("x", [
        { path: "project.eap", matches: [{ line: 1, preview: "x", ranges: [[0, 1]] }] },
      ]),
    );
    fireEvent.change(screen.getByRole("searchbox", { name: "내용 검색" }), {
      target: { value: "x" },
    });
    const header = await screen.findByRole("button", { name: "project.eap 결과 1개 접기" });
    fireEvent.click(header);

    view.rerender(
      <WorkspaceFileTree
        workspace={{ ...workspace, files: [...workspace.files] }}
        selectedPath={null}
        loading={false}
        {...handlers}
      />,
    );
    await new Promise((resolve) => setTimeout(resolve, 250));
    expect(handlers.onSearch).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "project.eap 결과 1개 펼치기" })).toBeInTheDocument();
  });

  it("ignores the response of a superseded query", async () => {
    const { handlers } = renderTree();
    let resolveOld: (value: WorkspaceSearchResponse) => void = () => {};
    handlers.onSearch.mockImplementation((query: string) =>
      query === "old"
        ? new Promise<WorkspaceSearchResponse>((resolve) => {
            resolveOld = resolve;
          })
        : Promise.resolve(searchResponse(query, [])),
    );
    const search = screen.getByRole("searchbox", { name: "내용 검색" });
    fireEvent.change(search, { target: { value: "old" } });
    await waitFor(() => expect(handlers.onSearch).toHaveBeenCalledWith("old"));
    fireEvent.change(search, { target: { value: "new" } });
    expect(await screen.findByText("검색 결과가 없습니다.")).toBeInTheDocument();

    resolveOld(
      searchResponse("old", [
        { path: "project.eap", matches: [{ line: 1, preview: "old", ranges: [[0, 3]] }] },
      ]),
    );
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByText("검색 결과가 없습니다.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "검색 결과" })).not.toBeInTheDocument();
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
