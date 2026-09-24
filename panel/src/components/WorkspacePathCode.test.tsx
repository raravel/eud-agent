import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AgentAnswer } from "@/components/AgentAnswer";
import { WorkspacePathProvider } from "@/components/WorkspacePathCode";
import { resolveMentionedWorkspacePath } from "@/components/workspaceLinks";
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
