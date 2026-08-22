import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SessionMeta } from "@/lib/protocol";
import { MapSessionHistoryDialog } from "./MapSessionHistoryDialog";

const sessions: SessionMeta[] = [
  {
    id: "session-current",
    name: "본진 지형 작업",
    project: "map-project",
    kind: "map",
    createdAt: 100,
    lastConversationAt: 2_000,
  },
  {
    id: "session-previous",
    name: "멀티 배치 검토",
    project: "map-project",
    kind: "map",
    createdAt: 50,
    lastConversationAt: 1_000,
  },
];

const baseProps = {
  open: true,
  sessions,
  activeId: "session-current",
  onOpenChange: vi.fn(),
  onReload: vi.fn(),
  onCreate: vi.fn(),
  onLoad: vi.fn(),
  onRename: vi.fn(),
  onDelete: vi.fn(),
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("MapSessionHistoryDialog", () => {
  it("shows the current and previous map sessions and loads the selected history", () => {
    const onLoad = vi.fn();
    render(<MapSessionHistoryDialog {...baseProps} onLoad={onLoad} />);

    expect(
      screen.getByRole("dialog", { name: "맵 작업 히스토리" }),
    ).toBeInTheDocument();
    expect(screen.getByText("현재 작업 ·", { exact: false })).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: "멀티 배치 검토 불러오기" }),
    );
    expect(onLoad).toHaveBeenCalledWith("session-previous");
    expect(
      screen.getByRole("button", { name: "본진 지형 작업 삭제" }),
    ).toBeDisabled();
  });

  it("filters history names and starts a separate map session", () => {
    const onCreate = vi.fn();
    render(<MapSessionHistoryDialog {...baseProps} onCreate={onCreate} />);

    fireEvent.change(screen.getByRole("textbox", { name: "맵 작업 검색" }), {
      target: { value: "멀티" },
    });
    expect(screen.queryByText("본진 지형 작업")).not.toBeInTheDocument();
    expect(screen.getByText("멀티 배치 검토")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "새 작업" }));
    expect(onCreate).toHaveBeenCalledOnce();
  });

  it("renames and deletes only an inactive history entry", () => {
    vi.spyOn(window, "prompt").mockReturnValue("수정된 작업 이름");
    vi.spyOn(window, "confirm").mockReturnValue(true);
    const onRename = vi.fn();
    const onDelete = vi.fn();
    render(
      <MapSessionHistoryDialog
        {...baseProps}
        onRename={onRename}
        onDelete={onDelete}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "멀티 배치 검토 이름 변경" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "멀티 배치 검토 삭제" }),
    );

    expect(onRename).toHaveBeenCalledWith(
      "session-previous",
      "수정된 작업 이름",
    );
    expect(onDelete).toHaveBeenCalledWith("session-previous");
  });
});
