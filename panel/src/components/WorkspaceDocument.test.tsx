import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { WorkspaceDocument } from "./WorkspaceDocument";
import type { WorkspaceListResponse } from "@/lib/ipc";

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: "specs/combat.md", size: 32 },
    { path: "worklog/req-1.md", size: 48 },
  ],
};

function entry(path: string) {
  return workspace.files.find((file) => file.path === path) ?? null;
}

function renderDocument(
  overrides: Partial<Parameters<typeof WorkspaceDocument>[0]> = {},
) {
  const onSelect = vi.fn();
  const view = render(
    <WorkspaceDocument
      workspace={workspace}
      file={entry("specs/combat.md")}
      content={"# Combat specification\n\nConfirmed behavior."}
      loading={false}
      error={null}
      onSelect={onSelect}
      {...overrides}
    />,
  );
  return { onSelect, view };
}

describe("WorkspaceDocument", () => {
  it("renders Markdown content with its trust label", () => {
    const { view } = renderDocument();
    expect(view.container).toHaveTextContent("Combat specification");
    expect(screen.getByText("검토 대상 문서")).toBeInTheDocument();
  });

  it("renders parent-owned acceptance metadata separately from document text", () => {
    const accepted = { ...workspace.files[0], state: "accepted", revision: 3 };
    renderDocument({
      workspace: { ...workspace, files: [accepted] },
      file: accepted,
      content: "# Accepted",
    });
    expect(screen.getByText("확정됨 · r3")).toBeInTheDocument();
  });

  it("renders authoritative approval metadata for saved plans", () => {
    renderDocument({
      workspace: {
        ...workspace,
        files: [
          {
            path: "plans/req-1.md",
            size: 24,
            state: "approved",
            revision: 2,
          },
        ],
      },
      file: {
        path: "plans/req-1.md",
        size: 24,
        state: "approved",
        revision: 2,
      },
      content: "# Approved plan",
    });
    expect(screen.getByText("승인된 계획 · r2")).toBeInTheDocument();
  });

  it("reports loading and read failures accessibly", () => {
    const { view } = renderDocument({ content: null, loading: true });
    expect(screen.getByText("파일을 여는 중…")).toBeInTheDocument();

    view.rerender(
      <WorkspaceDocument
        workspace={workspace}
        file={entry("specs/combat.md")}
        content={null}
        loading={false}
        error="읽기 실패"
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("읽기 실패");
  });

  it("navigates relative Markdown links through the file tree", async () => {
    const wikiWorkspace: WorkspaceListResponse = {
      ...workspace,
      files: [
        { path: "specs/index.md", size: 48 },
        ...workspace.files,
      ],
    };
    const { onSelect } = renderDocument({
      workspace: wikiWorkspace,
      file: wikiWorkspace.files[0],
      content: "# Project wiki\n\nOpen [Combat](combat.md).\n",
    });

    fireEvent.click(await screen.findByRole("link", { name: "Combat" }));
    expect(onSelect).toHaveBeenCalledWith(wikiWorkspace.files[1]);
  });
});
