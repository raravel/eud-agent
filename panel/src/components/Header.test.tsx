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
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
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
