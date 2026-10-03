import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SessionSidebar, type SessionSidebarProps } from "./SessionSidebar";

function props(overrides: Partial<SessionSidebarProps> = {}): SessionSidebarProps {
  return {
    project: "ExampleProject",
    rows: [
      {
        id: "running",
        name: "유닛 밸런스",
        lastConversationAt: 10_000,
        activity: "running_read",
        persisted: true,
      },
      {
        id: "writing",
        name: "트리거 수정",
        lastConversationAt: 9_000,
        activity: "running_write",
        persisted: true,
      },
      {
        id: "review",
        name: "맵 설정",
        lastConversationAt: 8_000,
        activity: "review",
        persisted: true,
      },
    ],
    selectedId: "writing",
    collapsed: false,
    onCollapsedChange: vi.fn(),
    onNew: vi.fn(),
    onSelect: vi.fn(),
    onRename: vi.fn(),
    onDelete: vi.fn(),
    ...overrides,
  };
}

beforeEach(() => {
  localStorage.clear();
});

describe("SessionSidebar", () => {
  it("toggles worker groups without changing the selected conversation and reveals search matches", () => {
    const handlers = props();
    handlers.rows = [
      { id: "child", name: "지도 조사", lastConversationAt: 50_000, provider: "codex",
        activity: "running_read", persisted: true, workerParentId: "writing", workerParentName: "트리거 수정" },
      ...handlers.rows,
    ];
    const { rerender } = render(<SessionSidebar {...handlers} />);
    const collapse = screen.getByRole("button", { name: "트리거 수정 작업자 접기" });
    expect(collapse).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("list", { name: "트리거 수정 작업자" })).toHaveAttribute(
      "id", collapse.getAttribute("aria-controls"),
    );
    fireEvent.click(collapse);
    expect(handlers.onSelect).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "지도 조사, 작업자, 분석 중" })).not.toBeInTheDocument();
    rerender(<SessionSidebar {...handlers} />);
    const expand = screen.getByRole("button", { name: "트리거 수정 작업자 펼치기" });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(expand);
    expect(screen.getByRole("button", { name: "지도 조사, 작업자, 분석 중" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "트리거 수정 작업자 접기" }));
    rerender(<SessionSidebar {...handlers} collapsed />);
    expect(screen.queryByRole("list", { name: "트리거 수정 작업자" })).not.toBeInTheDocument();
    rerender(<SessionSidebar {...handlers} />);
    expect(screen.getByRole("button", { name: "트리거 수정 작업자 펼치기" })).toHaveAttribute("aria-expanded", "false");
    fireEvent.change(screen.getByRole("textbox", { name: "세션 검색" }), { target: { value: "지도" } });
    expect(screen.getByRole("button", { name: "지도 조사, 작업자, 분석 중" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "트리거 수정 작업자 접기" })).toHaveAttribute("aria-expanded", "true");
  });

  it("nests a newer worker beneath its parent and retains that parent in search", () => {
    const handlers = props();
    handlers.rows = [
      { id: "child", name: "지도 조사", lastConversationAt: 50_000, provider: "codex",
        activity: "idle", persisted: true, workerParentId: "writing", workerParentName: "트리거 수정" },
      ...handlers.rows,
    ];
    render(<SessionSidebar {...handlers} />);
    const children = screen.getByRole("list", { name: "트리거 수정 작업자" });
    const child = within(children).getByRole("button", { name: "지도 조사, 작업자, 유휴" });
    fireEvent.click(child);
    expect(handlers.onSelect).toHaveBeenCalledWith("child");
    expect(children.parentElement?.firstElementChild).toContainElement(
      screen.getByRole("button", { name: "트리거 수정, 변경 중" }),
    );
    fireEvent.change(screen.getByRole("textbox", { name: "세션 검색" }), { target: { value: "지도" } });
    expect(screen.getByRole("button", { name: "트리거 수정, 변경 중" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "트리거 수정 작업자" })).toContainElement(child);
    expect(screen.queryByRole("button", { name: "유닛 밸런스, 분석 중" })).not.toBeInTheDocument();
  });
  it("distinguishes selected, running, writing, and review states", () => {
    render(<SessionSidebar {...props()} />);

    expect(
      screen.getByRole("button", { name: "트리거 수정, 변경 중" }),
    ).toHaveAttribute("aria-current", "page");
    expect(screen.getByText("분석 중")).toBeInTheDocument();
    expect(screen.getByText("변경 중")).toBeInTheDocument();
    expect(screen.getByText("검토 필요")).toBeInTheDocument();
  });

  it("selects a running row", () => {
    const handlers = props();
    render(<SessionSidebar {...handlers} />);

    fireEvent.click(screen.getByRole("button", { name: "유닛 밸런스, 분석 중" }));
    expect(handlers.onSelect).toHaveBeenCalledWith("running");
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
              lastConversationAt: 10_000,
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

  it("shows the last conversation time beside an idle session", () => {
    const lastConversationAt = Date.UTC(2026, 7, 21, 9, 30);
    render(
      <SessionSidebar
        {...props({
          rows: [
            {
              id: "idle",
              name: "최근 대화",
              lastConversationAt,
              activity: "idle",
              persisted: true,
            },
          ],
          selectedId: "idle",
        })}
      />,
    );

    const formatted = new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(new Date(lastConversationAt));
    expect(screen.getByText(`· ${formatted}`)).toBeInTheDocument();
  });

  it("shows each session's short id and copies the full id from the row", () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const fullId = "a3f9c2d1-7b1e-4c2a-9d0f-1234567890ab";
    render(
      <SessionSidebar
        {...props({
          rows: [
            {
              id: fullId,
              name: "짧은 ID 확인",
              lastConversationAt: 10_000,
              activity: "idle",
              persisted: true,
            },
          ],
          selectedId: fullId,
        })}
      />,
    );

    expect(screen.getByText("a3f9c2d1")).toBeInTheDocument();
    expect(screen.queryByText(fullId)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "짧은 ID 확인 세션 ID 복사" }));
    expect(writeText).toHaveBeenCalledWith(fullId);
    Reflect.deleteProperty(navigator, "clipboard");
  });

  it("hides the copy action while collapsed", () => {
    render(<SessionSidebar {...props({ collapsed: true })} />);

    expect(
      screen.queryByRole("button", { name: "트리거 수정 세션 ID 복사" }),
    ).not.toBeInTheDocument();
  });
});
