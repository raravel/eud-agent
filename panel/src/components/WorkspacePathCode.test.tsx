import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AgentAnswer } from "@/components/AgentAnswer";
import { CHAT_MARKDOWN_COMPONENTS, WorkspacePathProvider } from "@/components/WorkspacePathCode";
import { DiagramResponse } from "@/components/ai-elements/response";
import { resolveChatFileLink, resolveMentionedWorkspacePath } from "@/components/workspaceLinks";
import type { WorkspaceFileEntry } from "@/lib/ipc";

const files: WorkspaceFileEntry[] = [
  { path: ".eud-agent/workspace/specs/opening.md", size: 10 },
  { path: "src/main.eps", size: 20 },
];

describe("resolveMentionedWorkspacePath", () => {
  it("matches listed project-relative paths in the forms the agent writes", () => {
    for (const text of [
      ".eud-agent/workspace/specs/opening.md",
      "./.eud-agent/workspace/specs/opening.md",
      ".eud-agent\\workspace\\specs\\opening.md",
      "specs/opening.md",
    ]) {
      expect(resolveMentionedWorkspacePath(files, text)?.path).toBe(
        ".eud-agent/workspace/specs/opening.md",
      );
    }
    expect(resolveMentionedWorkspacePath(files, "src/main.eps:42")?.path).toBe("src/main.eps");
    expect(resolveMentionedWorkspacePath(files, "src/main.eps:42:7")?.path).toBe("src/main.eps");
  });

  it("rejects paths that are not exactly a listed file", () => {
    for (const text of ["src/missing.eps", "main.eps", "/src/main.eps", "C:/src/main.eps", "", "dat"]) {
      expect(resolveMentionedWorkspacePath(files, text)).toBeNull();
    }
  });
});

describe("chat answer workspace paths", () => {
  it("resolves encoded source citations but rejects other projects and malformed paths", () => {
    const root = "E:/proj/eud/proj1/defense";
    expect(resolveChatFileLink(files, `file:///${root}/src/main.eps:42:7#L42`, root)).toBe(files[1]);
    expect(resolveChatFileLink(files, `/e:/PROJ/eud/proj1/defense/src/main.eps`, root)).toBe(files[1]);
    expect(resolveChatFileLink(files, `${root}/src/%6dain.eps`, root)).toBe(files[1]);
    for (const href of [
      "E:/other/src/main.eps",
      `${root}-other/src/main.eps`,
      `${root}/../other/src/main.eps`,
      `${root}/src/missing.eps`,
      "src/%broken.eps",
    ]) {
      expect(resolveChatFileLink(files, href, root)).toBeNull();
    }
  });

  it("opens reference-style local Markdown links in archived answers", () => {
    const onOpen = vi.fn();
    render(
      <WorkspacePathProvider files={files} onOpen={onOpen}>
        <DiagramResponse mode="static" components={CHAT_MARKDOWN_COMPONENTS}>
          {"[작업 문서][spec]\n\n[spec]: specs/opening.md"}
        </DiagramResponse>
      </WorkspacePathProvider>,
    );
    fireEvent.click(screen.getByRole("link", { name: "작업 문서" }));
    expect(onOpen).toHaveBeenCalledExactlyOnceWith(files[0]);
  });

  it("does not send missing local citations to the browser", () => {
    render(
      <WorkspacePathProvider files={files} onOpen={vi.fn()}>
        <AgentAnswer text="[없는 파일](src/missing.eps)" />
      </WorkspacePathProvider>,
    );
    expect(screen.getByText("없는 파일")).toBeInTheDocument();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it.each([
    "/E:/proj/eud/proj1/defense/.eud-agent/workspace/specs/opening.md",
    "E:/proj/eud/proj1/defense/.eud-agent/workspace/specs/opening.md",
    "file:///E:/proj/eud/proj1/defense/.eud-agent/workspace/specs/opening.md",
    ".eud-agent/workspace/specs/opening.md",
    "specs/opening.md",
  ])("opens Markdown file link %s without reaching the shell browser listener", (href) => {
    const onOpen = vi.fn();
    const shellClick = vi.fn();
    document.body.addEventListener("click", shellClick);
    try {
      render(
        <WorkspacePathProvider files={files} projectRoot="E:/proj/eud/proj1/defense" onOpen={onOpen}>
          <AgentAnswer text={`[작업 문서](${href})`} />
        </WorkspacePathProvider>,
      );
      const link = screen.getByRole("link", { name: "작업 문서" });
      expect(fireEvent.click(link)).toBe(false);
      expect(onOpen).toHaveBeenCalledExactlyOnceWith(files[0]);
      expect(shellClick).not.toHaveBeenCalled();
    } finally {
      document.body.removeEventListener("click", shellClick);
    }
  });

  it("keeps web citations as external links", () => {
    render(
      <WorkspacePathProvider files={files} onOpen={vi.fn()}>
        <AgentAnswer text="[공식 문서](https://example.com/docs)" />
      </WorkspacePathProvider>,
    );
    const link = screen.getByRole("link", { name: "공식 문서" });
    expect(link).toHaveAttribute("href", "https://example.com/docs");
    expect(link).toHaveAttribute("target", "_blank");
  });

  it("opens a listed file from its inline-code path and leaves other code plain", () => {
    const onOpen = vi.fn();
    render(
      <WorkspacePathProvider files={files} onOpen={onOpen}>
        <AgentAnswer text="`.eud-agent/workspace/specs/opening.md` 여기에 작업했습니다. `SetDeaths` 사용." />
      </WorkspacePathProvider>,
    );

    fireEvent.click(
      screen.getByRole("button", { name: ".eud-agent/workspace/specs/opening.md 파일 열기" }),
    );
    expect(onOpen).toHaveBeenCalledWith(files[0]);
    expect(screen.getByText("SetDeaths").closest("button")).toBeNull();
    expect(screen.getAllByRole("button")).toHaveLength(1);
  });

  it("renders plain inline code without a provider", () => {
    render(<AgentAnswer text="`src/main.eps` 수정" />);
    expect(screen.getByText("src/main.eps").tagName).toBe("CODE");
    expect(screen.queryByRole("button")).toBeNull();
  });
});
