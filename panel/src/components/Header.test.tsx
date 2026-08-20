/**
 * Header (features/06 ## Behaviors → Status visibility): title, project name
 * (from status), connection-state transitions (연결 중 → 연결됨 → 재연결 중),
 * and RAG model state with elapsed seconds while loading. Korean labels.
 *
 * Contract (Step B implements `@/components/Header`):
 *   export type RagState = "idle" | "loading" | "ready" | "unavailable";
 *   export interface HeaderProps {
 *     project: string;        // "" when no project / unknown
 *     connected: boolean;     // store.connected (derived)
 *     phase: Phase;           // for connecting/retry wording
 *     rag?: { state: RagState; elapsedSec?: number };  // RAG visibility
 *   }
 *   export function Header(props): JSX.Element;
 *
 * Connection wording (Korean): connected → "연결됨"; phase "retry" →
 * "재연결 중…"; otherwise (connecting) → "연결 중…".
 * RAG wording: loading → "RAG: 로드 중 {n}초" (elapsed via progress.formatElapsed);
 * ready → "RAG: 준비됨"; unavailable → "RAG: 불가"; idle → no RAG pill.
 */
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Header } from "@/components/Header";

describe("Header — title + project + connection", () => {
  it("shows the app title in Korean", () => {
    render(<Header project="" connected={false} phase="connecting" />);
    expect(screen.getByText("EUD 에이전트")).toBeInTheDocument();
  });

  it("shows the project name from status when present", () => {
    render(<Header project="MyMap" connected={true} phase="ready" />);
    expect(screen.getByText("MyMap")).toBeInTheDocument();
  });

  it("shows '연결됨' when connected", () => {
    render(<Header project="MyMap" connected={true} phase="ready" />);
    expect(screen.getByText("연결됨")).toBeInTheDocument();
  });

  it("shows '재연결 중…' in the retry phase", () => {
    render(<Header project="" connected={false} phase="retry" />);
    expect(screen.getByText("재연결 중…")).toBeInTheDocument();
  });

  it("shows '연결 중…' while connecting", () => {
    render(<Header project="" connected={false} phase="connecting" />);
    expect(screen.getByText("연결 중…")).toBeInTheDocument();
  });
});

describe("Header — connection chip no-project suffix", () => {
  it("appends '· 프로젝트 없음' when the editor is connected but no project is open", () => {
    render(
      <Header
        project=""
        connected={true}
        phase="ready"
        editorConnected={true}
        hasProject={false}
      />,
    );
    expect(screen.getByText("연결됨 · 프로젝트 없음")).toBeInTheDocument();
  });

  it("shows plain '연결됨' when a project is open", () => {
    render(
      <Header
        project="MyMap"
        connected={true}
        phase="ready"
        editorConnected={true}
        hasProject={true}
      />,
    );
    expect(screen.getByText("연결됨")).toBeInTheDocument();
    expect(screen.queryByText(/프로젝트 없음/)).not.toBeInTheDocument();
  });

  it("does not append the suffix when the editor is disconnected (banner covers it)", () => {
    render(
      <Header
        project=""
        connected={true}
        phase="ready"
        editorConnected={false}
        hasProject={false}
      />,
    );
    expect(screen.getByText("연결됨")).toBeInTheDocument();
    expect(screen.queryByText(/프로젝트 없음/)).not.toBeInTheDocument();
  });
});

describe("Header — RAG state visibility", () => {
  it("shows no RAG pill when idle (or rag prop omitted)", () => {
    render(<Header project="" connected={true} phase="ready" />);
    expect(screen.queryByText(/RAG/)).not.toBeInTheDocument();
  });

  it("shows the elapsed seconds while RAG is loading", () => {
    render(
      <Header
        project=""
        connected={true}
        phase="ready"
        rag={{ state: "loading", elapsedSec: 7 }}
      />,
    );
    const pill = screen.getByText(/RAG/);
    expect(pill.textContent).toContain("로드 중");
    expect(pill.textContent).toContain("7");
  });

  it("shows '준비됨' when RAG is ready", () => {
    render(
      <Header
        project=""
        connected={true}
        phase="ready"
        rag={{ state: "ready" }}
      />,
    );
    expect(screen.getByText(/준비됨/)).toBeInTheDocument();
  });

  it("shows '불가' when RAG is unavailable", () => {
    render(
      <Header
        project=""
        connected={true}
        phase="ready"
        rag={{ state: "unavailable" }}
      />,
    );
    expect(screen.getByText(/불가/)).toBeInTheDocument();
  });
});

describe("Header — 에디터 켜기 button", () => {
  it("is hidden when no launch handler is provided", () => {
    render(<Header project="" connected={false} phase="connecting" />);
    expect(
      screen.queryByRole("button", { name: "에디터 켜기" }),
    ).not.toBeInTheDocument();
  });

  it("launches the editor on click when disconnected", async () => {
    const onLaunchEditor = vi.fn();
    render(
      <Header
        project=""
        connected={false}
        phase="connecting"
        editorConnected={false}
        onLaunchEditor={onLaunchEditor}
      />,
    );
    const button = screen.getByRole("button", { name: "에디터 켜기" });
    expect(button).toBeEnabled();
    await userEvent.click(button);
    expect(onLaunchEditor).toHaveBeenCalledTimes(1);
  });

  it("is disabled when the editor is already connected (no re-launch)", () => {
    render(
      <Header
        project="MyMap"
        connected={true}
        phase="ready"
        editorConnected={true}
        onLaunchEditor={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "에디터 켜기" }),
    ).toBeDisabled();
  });

  it("shows a pending label and disables while a launch is in flight", () => {
    render(
      <Header
        project=""
        connected={false}
        phase="connecting"
        editorConnected={false}
        launchPending={true}
        onLaunchEditor={vi.fn()}
      />,
    );
    expect(
      screen.getByRole("button", { name: "여는 중…" }),
    ).toBeDisabled();
  });
});

describe("Header — project tools sidebar", () => {
  it("renders one toggle for the sidebar and calls its handler", async () => {
    const onProjectPanelToggle = vi.fn();
    render(
      <Header
        project="MyMap"
        connected={true}
        phase="ready"
        projectPanelOpen={true}
        onProjectPanelToggle={onProjectPanelToggle}
      />,
    );

    const button = screen.getByRole("button", {
      name: "프로젝트 도구 닫기",
    });
    expect(button).toHaveAttribute("aria-pressed", "true");
    expect(
      screen.queryByRole("button", { name: "프로젝트 워크스페이스" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "dat 편집 위키" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "메모리" }),
    ).not.toBeInTheDocument();

    await userEvent.click(button);
    expect(onProjectPanelToggle).toHaveBeenCalledTimes(1);
  });
});
