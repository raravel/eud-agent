import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { WorkspaceDocument } from "./WorkspaceDocument";
import type { WorkspaceListResponse } from "@/lib/ipc";

// Monaco is module-mocked with a textarea double (vitest.config.ts): the
// viewer's contract is which surface it picks and with what props.
vi.mock("@/components/MonacoEditor", async () => {
  const React = await import("react");
  function MonacoEditor({
    value = "",
    language,
    readOnly,
    ariaLabel,
  }: {
    value?: string;
    language?: string;
    readOnly?: boolean;
    ariaLabel?: string;
  }) {
    return React.createElement("textarea", {
      "aria-label": ariaLabel,
      "data-language": language,
      readOnly,
      value,
      onChange: () => {},
    });
  }
  return { default: MonacoEditor, MonacoEditor };
});

const D = ".eud-agent/workspace/";

const workspace: WorkspaceListResponse = {
  project: "Example",
  workspaceId: "a".repeat(64),
  files: [
    { path: "src/main.eps", size: 40 },
    { path: "src/patch.py", size: 20 },
    { path: "maps/source.scx", size: 2048 },
    { path: `${D}specs/combat.md`, size: 32 },
    { path: `${D}worklog/req-1.md`, size: 48 },
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
      path={`${D}specs/combat.md`}
      file={entry(`${D}specs/combat.md`)}
      content={"# Combat specification\n\nConfirmed behavior."}
      loading={false}
      error={null}
      notice={null}
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
    const accepted = { ...workspace.files[2], state: "accepted", revision: 3 };
    renderDocument({
      workspace: { ...workspace, files: [accepted] },
      file: accepted,
      content: "# Accepted",
    });
    expect(screen.getByText("확정됨 · r3")).toBeInTheDocument();
  });

  it("renders authoritative approval metadata for saved plans", () => {
    const plan = {
      path: `${D}plans/req-1.md`,
      size: 24,
      state: "approved",
      revision: 2,
    };
    renderDocument({
      workspace: { ...workspace, files: [plan] },
      path: plan.path,
      file: plan,
      content: "# Approved plan",
    });
    expect(screen.getByText("승인된 계획 · r2")).toBeInTheDocument();
  });

  it("renders a stage report as Markdown from its tab path even when unlisted", () => {
    const { view } = renderDocument({
      path: `${D}research/req-1.md`,
      file: null,
      content: "# 조사 보고\n\n트리거 3개를 확인했습니다.",
    });
    expect(view.container.querySelector("pre")).toBeNull();
    expect(screen.getByRole("heading", { name: "조사 보고" })).toBeInTheDocument();
    expect(screen.getByText(`${D}research/req-1.md`)).toBeInTheDocument();
    expect(screen.getByText("작업 보고서")).toBeInTheDocument();
  });

  it("opens EPS source in a read-only Monaco surface with the TypeScript grammar", async () => {
    const { view } = renderDocument({
      path: "src/main.eps",
      file: entry("src/main.eps"),
      content: "function onPluginStart() {}",
    });
    const editor = await screen.findByRole("textbox", { name: "src/main.eps 소스" });
    expect(editor).toHaveValue("function onPluginStart() {}");
    expect(editor).toHaveAttribute("data-language", "typescript");
    expect(editor).toHaveAttribute("readonly");
    expect(view.container.querySelector("pre")).toBeNull();
    expect(screen.getByText("프로젝트 파일")).toBeInTheDocument();
  });

  it("renders other project text files as preformatted source", () => {
    const { view } = renderDocument({
      path: "src/patch.py",
      file: entry("src/patch.py"),
      content: "print('x')",
    });
    expect(view.container.querySelector("pre")).toHaveTextContent("print('x')");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText("프로젝트 파일")).toBeInTheDocument();
  });

  it("shows the closed-file notice instead of content or an error", () => {
    renderDocument({
      path: "maps/source.scx",
      file: entry("maps/source.scx"),
      content: null,
      notice: "텍스트로 표시할 수 없는 파일입니다 (바이너리 · 2 KB).",
    });
    expect(screen.getByRole("status")).toHaveTextContent("바이너리 · 2 KB");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText("파일 트리에서 문서를 선택하세요.")).not.toBeInTheDocument();
  });

  it("reports loading and read failures accessibly", () => {
    const { view } = renderDocument({ content: null, loading: true });
    expect(screen.getByText("파일을 여는 중…")).toBeInTheDocument();

    view.rerender(
      <WorkspaceDocument
        workspace={workspace}
        path={`${D}specs/combat.md`}
        file={entry(`${D}specs/combat.md`)}
        content={null}
        loading={false}
        error="읽기 실패"
        notice={null}
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("읽기 실패");
  });

  it("navigates relative Markdown links through the file tree", async () => {
    const wikiWorkspace: WorkspaceListResponse = {
      ...workspace,
      files: [
        { path: `${D}specs/index.md`, size: 48 },
        ...workspace.files,
      ],
    };
    const { onSelect } = renderDocument({
      workspace: wikiWorkspace,
      path: `${D}specs/index.md`,
      file: wikiWorkspace.files[0],
      content: "# Project wiki\n\nOpen [Combat](combat.md).\n",
    });

    fireEvent.click(await screen.findByRole("link", { name: "Combat" }));
    expect(onSelect).toHaveBeenCalledWith(entry(`${D}specs/combat.md`));
  });
});
