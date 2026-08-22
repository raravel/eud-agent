import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ImagePlacementControls } from "./ImagePlacementControls";
import type { MapImageConversionReport } from "./mapProtocol";

const placement = { x: 4, y: 5, width: 16, height: 8 };
const report: MapImageConversionReport = {
  sourceDimensions: { width: 400, height: 200 },
  placement,
  changedCells: 90,
  changedRows: [{ y: 5, spans: [[4, 20]] }],
  uniqueTileCount: 12,
  walkabilityChangedCells: 7,
  heightChangedCells: 3,
  protectedConflicts: 0,
  outsideAuthorityConflicts: 0,
  tileGridSha256: "digest",
  quantizerVersion: "sd-bayer8-v1",
};

function props(overrides: Partial<React.ComponentProps<typeof ImagePlacementControls>> = {}) {
  return {
    fileName: "terrain.png",
    sourceDimensions: { width: 400, height: 200 },
    placement,
    mapWidth: 128,
    mapHeight: 128,
    previewMode: "original" as const,
    report,
    previewFresh: true,
    previewLoading: false,
    confirming: false,
    onPlacement: vi.fn(),
    onPreviewMode: vi.fn(),
    onConfirm: vi.fn(),
    onCancel: vi.fn(),
    ...overrides,
  };
}

afterEach(() => {
  vi.useRealTimers();
});

describe("ImagePlacementControls", () => {
  it("shows conversion and gameplay reports and enables only a fresh safe confirm", () => {
    const onConfirm = vi.fn();
    const { rerender } = render(
      <ImagePlacementControls {...props({ onConfirm })} />,
    );
    expect(screen.getByText("변경 90셀")).toBeInTheDocument();
    expect(screen.getByText("고유 타일 12")).toBeInTheDocument();
    expect(screen.getByText("보행 변화 7")).toBeInTheDocument();
    expect(screen.getByText("고도 변화 3")).toBeInTheDocument();
    const confirm = screen.getByRole("button", { name: "후보에 반영" });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    expect(onConfirm).toHaveBeenCalledOnce();

    rerender(<ImagePlacementControls {...props({ previewFresh: false })} />);
    expect(screen.getByRole("button", { name: "후보에 반영" })).toBeDisabled();
    rerender(
      <ImagePlacementControls
        {...props({ report: { ...report, protectedConflicts: 1 } })}
      />,
    );
    expect(screen.getByRole("button", { name: "후보에 반영" })).toBeDisabled();
  });

  it("switches original/result views and debounces aspect-locked numeric edits", () => {
    vi.useFakeTimers();
    const onPreviewMode = vi.fn();
    const onPlacement = vi.fn();
    render(
      <ImagePlacementControls
        {...props({ onPreviewMode, onPlacement })}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "적용 결과" }));
    expect(onPreviewMode).toHaveBeenCalledWith("result");

    const width = screen.getByLabelText("너비");
    fireEvent.change(width, { target: { value: "20" } });
    expect(onPlacement).toHaveBeenLastCalledWith(
      { x: 4, y: 5, width: 20, height: 10 },
      false,
    );
    vi.advanceTimersByTime(180);
    expect(onPlacement).toHaveBeenLastCalledWith(
      { x: 4, y: 5, width: 20, height: 10 },
      true,
    );
  });

  it("provides visible cancel and keyboard movement instructions", () => {
    const onCancel = vi.fn();
    render(<ImagePlacementControls {...props({ onCancel })} />);
    expect(screen.getByText(/Shift\+방향키 8타일/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });
});
