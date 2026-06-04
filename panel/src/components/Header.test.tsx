/**
 * Header (features/03 ## UI layout): title, project name (from status),
 * connection state. Korean labels throughout.
 *
 * Contract (Step B implements `@/components/Header`):
 *   export interface HeaderProps {
 *     project: string;     // "" when no project / unknown
 *     connected: boolean;  // store.connected (derived)
 *     phase: Phase;        // for connecting/retry wording
 *   }
 *   export function Header(props): JSX.Element;
 *
 * Connection wording (Korean): connected → "연결됨"; phase "retry" →
 * "재연결 대기 중…"; otherwise (connecting) → "연결 중…".
 */
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Header } from "@/components/Header";

describe("Header", () => {
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

  it("shows '재연결 대기 중…' in the retry phase", () => {
    render(<Header project="" connected={false} phase="retry" />);
    expect(screen.getByText("재연결 대기 중…")).toBeInTheDocument();
  });

  it("shows '연결 중…' while connecting", () => {
    render(<Header project="" connected={false} phase="connecting" />);
    expect(screen.getByText("연결 중…")).toBeInTheDocument();
  });
});
