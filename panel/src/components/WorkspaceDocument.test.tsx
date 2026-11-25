import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

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

vi.mock("@tauri-apps/api/core", async (original) => ({
  ...(await original<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) =>
    `http://${protocol}.localhost/${encodeURIComponent(path)}`,
}));

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
  const onOpenLink = vi.fn();
  const view = render(
    <WorkspaceDocument
      workspace={workspace}
      path={`${D}specs/combat.md`}
      file={entry(`${D}specs/combat.md`)}
      active
      content={"# Combat specification\n\nConfirmed behavior."}
      loading={false}
      error={null}
      notice={null}
      onSelect={onSelect}
      onOpenLink={onOpenLink}
      {...overrides}
    />,
  );
  return { onSelect, onOpenLink, view };
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

  it("renders an unlisted workspace document as Markdown from its tab path", () => {
    const { view } = renderDocument({
      path: `${D}decisions/req-1.md`,
      file: null,
      content: "# 조사 보고\n\n트리거 3개를 확인했습니다.",
    });
    expect(view.container.querySelector("pre")).toBeNull();
    expect(screen.getByRole("heading", { name: "조사 보고" })).toBeInTheDocument();
    expect(screen.getByText(`${D}decisions/req-1.md`)).toBeInTheDocument();
    expect(screen.getByText("검토 대상 문서")).toBeInTheDocument();
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
        active
        content={null}
        loading={false}
        error="읽기 실패"
        notice={null}
        onSelect={vi.fn()}
        onOpenLink={vi.fn()}
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

    // The Tauri shell plugin opens every target=_blank http(s) link clicked
    // anywhere under <body> in the system browser; a file link must not reach it.
    const shellOpen = vi.fn();
    document.body.addEventListener("click", shellOpen);
    try {
      fireEvent.click(await screen.findByRole("link", { name: "Combat" }));
    } finally {
      document.body.removeEventListener("click", shellOpen);
    }
    expect(onSelect).toHaveBeenCalledWith(entry(`${D}specs/combat.md`));
    expect(shellOpen).not.toHaveBeenCalled();
  });

  describe("HTML files", () => {
    // No protocol serves the frame here; happy-dom would try the network.
    const settings = () =>
      (window as unknown as { happyDOM: { settings: { disableIframePageLoading: boolean } } })
        .happyDOM.settings;
    beforeAll(() => {
      settings().disableIframePageLoading = true;
    });
    afterAll(() => {
      settings().disableIframePageLoading = false;
    });

    const page = { path: "docs/가이드 1.html", file: null, content: "<p>hi</p>" };
    const title = "docs/가이드 1.html 미리보기";

    it("renders the page in a frame without the app's origin", () => {
      renderDocument(page);
      const frame = screen.getByTitle(title);
      // Slashes stay, so the page's relative links resolve inside the project.
      expect(frame).toHaveAttribute(
        "src",
        `http://eudpreview.localhost/docs/${encodeURIComponent("가이드 1.html")}`,
      );
      const flags = (frame.getAttribute("sandbox") ?? "").split(" ");
      expect(flags).toContain("allow-scripts");
      expect(flags).not.toContain("allow-same-origin");
      expect(flags).not.toContain("allow-top-navigation");
      expect(flags).not.toContain("allow-popups");
    });

    it("renders a page the text viewer keeps closed, but never a failed read", () => {
      const { view } = renderDocument({ ...page, content: null, notice: "바이너리" });
      expect(screen.getByTitle(title)).toBeInTheDocument();
      view.unmount();
      renderDocument({ ...page, content: null, error: "읽기 실패" });
      expect(screen.queryByTitle(title)).not.toBeInTheDocument();
      expect(screen.getByRole("alert")).toHaveTextContent("읽기 실패");
    });

    it("switches to the highlighted source and back, and reloads the page", async () => {
      renderDocument(page);
      const before = screen.getByTitle(title);
      fireEvent.click(screen.getByRole("button", { name: "미리보기 새로 고침" }));
      expect(screen.getByTitle(title)).not.toBe(before);

      fireEvent.mouseDown(screen.getByRole("tab", { name: "소스" }));
      const editor = await screen.findByRole("textbox", { name: "docs/가이드 1.html 소스" });
      expect(editor).toHaveAttribute("data-language", "html");
      expect(screen.queryByTitle(title)).not.toBeInTheDocument();

      fireEvent.mouseDown(screen.getByRole("tab", { name: "미리보기" }));
      expect(screen.getByTitle(title)).toBeInTheDocument();
    });

    it("opens a search match in the source", async () => {
      renderDocument({ ...page, revealLine: { line: 3 } });
      expect(
        await screen.findByRole("textbox", { name: "docs/가이드 1.html 소스" }),
      ).toBeInTheDocument();
      expect(screen.queryByTitle(title)).not.toBeInTheDocument();
    });

    it("does not run the page of a tab that is not on screen", () => {
      renderDocument({ ...page, active: false });
      expect(screen.queryByTitle(title)).not.toBeInTheDocument();
      expect(screen.getByRole("tab", { name: "미리보기" })).toBeInTheDocument();
    });

    it("shows a page of a hidden folder as source only", async () => {
      renderDocument({ ...page, path: ".git/page.html" });
      expect(await screen.findByRole("textbox", { name: ".git/page.html 소스" })).toBeInTheDocument();
      expect(screen.queryByRole("tab", { name: "미리보기" })).not.toBeInTheDocument();
    });

    it("previews a page among the agent documents", () => {
      renderDocument({ ...page, path: `${D}specs/report.HTM` });
      expect(screen.getByTitle(`${D}specs/report.HTM 미리보기`)).toBeInTheDocument();
    });

    it("opens a page's outside link only from that frame and the user's click", () => {
      const { onOpenLink } = renderDocument(page);
      const frame = screen.getByTitle<HTMLIFrameElement>(title);
      const pageWindow = Object.create(null) as Window;
      Object.defineProperty(frame, "contentWindow", { value: pageWindow });
      const post = (href: unknown, source: Window = pageWindow) =>
        window.dispatchEvent(
          new MessageEvent("message", {
            data: { type: "eud-preview-link", href },
            source,
          }),
        );
      const activation = { isActive: false };
      Object.defineProperty(navigator, "userActivation", {
        configurable: true,
        value: activation,
      });
      try {
        post("https://example.com/a");
        expect(onOpenLink).not.toHaveBeenCalled();

        activation.isActive = true;
        post("https://example.com/a");
        expect(onOpenLink).not.toHaveBeenCalled();

        frame.focus();
        post("https://example.com/a", window);
        post("file:///C:/Windows/win.ini");
        post("javascript:alert(1)");
        post(1);
        expect(onOpenLink).not.toHaveBeenCalled();

        post("https://example.com/a");
        // The same activation does not open a burst of them.
        post("https://example.com/b");
        expect(onOpenLink).toHaveBeenCalledExactlyOnceWith("https://example.com/a");
      } finally {
        Reflect.deleteProperty(navigator, "userActivation");
      }
    });
  });

  it("renders a Mermaid code fence in a Markdown document as a diagram", async () => {
    const { view } = renderDocument({
      content: "# Flow\n\n```mermaid\ngraph TD\nA[시작] --> B[완료]\n```\n",
    });

    await waitFor(() => {
      expect(view.container.querySelector('[data-streamdown="mermaid"]')).not.toBeNull();
    });
  });
});
