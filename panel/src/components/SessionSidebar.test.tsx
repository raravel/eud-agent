import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionSidebar, type SessionSidebarProps } from "./SessionSidebar";

function props(overrides: Partial<SessionSidebarProps> = {}): SessionSidebarProps {
  return {
    project: "ExampleProject",
    rows: [
      {
        id: "running",
        name: "유닛 밸런스",
        updatedAt: 10,
        activity: "running_read",
        persisted: true,
      },
      {
        id: "queued",
        name: "트리거 수정",
        updatedAt: 9,
        activity: "waiting_write",
        queuePosition: 2,
        persisted: true,
      },
      {
        id: "review",
        name: "맵 설정",
        updatedAt: 8,
        activity: "review",
        persisted: true,
      },
    ],
    selectedId: "queued",
    collapsed: false,
    onCollapsedChange: vi.fn(),
    onNew: vi.fn(),
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onDelete: vi.fn(),
    onCancelQueued: vi.fn(),
    ...overrides,
  };
}

beforeEach(() => {
  localStorage.clear();
});

describe("SessionSidebar", () => {
  it("distinguishes selected, running, queued, and review states", () => {
    render(<SessionSidebar {...props()} />);

    expect(
      screen.getByRole("button", { name: "트리거 수정, 쓰기 대기 2" }),
    ).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("분석 중")).toBeInTheDocument();
    expect(screen.getByText("쓰기 대기 2")).toBeInTheDocument();
    expect(screen.getByText("검토 필요")).toBeInTheDocument();
  });

  it("selects a row and cancels only its queued request", () => {
    const handlers = props();
    render(<SessionSidebar {...handlers} />);

    fireEvent.click(screen.getByRole("button", { name: "유닛 밸런스, 분석 중" }));
    expect(handlers.onSelect).toHaveBeenCalledWith("running");

    fireEvent.click(screen.getByRole("button", { name: "트리거 수정 대기 취소" }));
    expect(handlers.onCancelQueued).toHaveBeenCalledWith("queued");
  });

  it("keeps the new-session action available when collapsed", () => {
    const handlers = props({ collapsed: true });
    render(<SessionSidebar {...handlers} />);

    fireEvent.click(screen.getByRole("button", { name: "새 세션" }));
    expect(handlers.onNew).toHaveBeenCalledOnce();
  });

  it("exposes a keyboard-accessible splitter and persists width changes", () => {
    render(<SessionSidebar {...props()} />);
    const splitter = screen.getByRole("separator", {
      name: "세션 사이드바 너비 조절",
    });

    expect(splitter).toHaveAttribute("aria-valuenow", "272");
    fireEvent.keyDown(splitter, { key: "ArrowRight" });
    expect(splitter).toHaveAttribute("aria-valuenow", "288");
    expect(localStorage.getItem("eud.session-sidebar.width")).toBe("288");
  });

  it("clips the sidebar horizontally and ellipsizes a long session name", () => {
    const longName =
      "아주 긴 세션 이름이 사이드바 너비보다 길어도 가로 스크롤을 만들지 않아야 합니다";
    render(
      <SessionSidebar
        {...props({
          rows: [
            {
              id: "long",
              name: longName,
              updatedAt: 10,
              activity: "idle",
              persisted: true,
            },
          ],
          selectedId: "long",
        })}
      />,
    );

    const navigation = screen.getByRole("navigation", {
      name: "현재 프로젝트 세션",
    });
    expect(navigation).toHaveClass("overflow-x-hidden");
    const name = screen.getByTitle(longName);
    expect(name).toHaveClass("truncate");
    expect(name.closest("li")).toHaveClass("overflow-hidden");
  });
});
