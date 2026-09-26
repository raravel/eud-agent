import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { SelectionToolbar } from "./SelectionToolbar";

const baseProps = {
  activeCells: new Set(["1,1", "2,1"]),
  shape: "rectangle" as const,
  operation: "replace" as const,
  role: "target" as const,
  allowedLayers: ["terrain" as const],
  label: "영역 A",
  interactionMode: "select" as const,
  savedSelections: [],
  onShape: vi.fn(),
  onOperation: vi.fn(),
  onRole: vi.fn(),
  onLayers: vi.fn(),
  onLabel: vi.fn(),
  onInteractionMode: vi.fn(),
  onCells: vi.fn(),
  onSave: vi.fn(),
  onMention: vi.fn(),
  onClear: vi.fn(),
  onLoadSelection: vi.fn(),
  onDeleteSelection: vi.fn(),
};

describe("Map selection visible controls", () => {
  it("offers shape, set operation, role, layers, and row-span alternatives", async () => {
    const { container } = render(<SelectionToolbar {...baseProps} />);
    expect(screen.getByLabelText("Shape")).toHaveAttribute("data-slot", "select-trigger");
    expect(screen.getByLabelText("Operation")).toHaveAttribute("data-slot", "select-trigger");
    expect(screen.getByRole("button", { name: "protect" })).toBeInTheDocument();
    expect(screen.getByLabelText("terrain")).toBeChecked();
    expect(screen.getByText("2 × 1 = 2 셀")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "영역 생성" })).toBeInTheDocument();
    expect(container.querySelector("select")).toBeNull();
    expect(container.querySelector('input[type="checkbox"]')).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "좌표/row-span 편집" }));
    expect(screen.getByLabelText(/Canonical row spans/)).toHaveValue("1:1-3");
  });

  it("caps saved-area chips and reaches the rest through a searchable picker", async () => {
    const saved = Array.from({ length: 10 }, (_, index) => ({
      id: `s${index}`,
      label: `영역${index}`,
      sourceRevision: "r0",
      role: "target" as const,
      layers: ["terrain" as const],
      bounds: { left: 0, top: 0, right: 1, bottom: 1 },
      selectedCells: index + 1,
      rows: [{ y: 0, spans: [[0, 1]] as [number, number][] }],
      snapshotHash: "h",
    }));
    const onLoadSelection = vi.fn();
    const onDeleteSelection = vi.fn();
    render(
      <SelectionToolbar
        {...baseProps}
        savedSelections={saved}
        onLoadSelection={onLoadSelection}
        onDeleteSelection={onDeleteSelection}
      />,
    );
    // Only the newest four are chips.
    expect(screen.getByRole("button", { name: "target:영역9 · 10" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "target:영역2 · 3" })).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /전체 영역 10개/ }));
    await userEvent.type(screen.getByLabelText("저장된 영역 검색"), "영역2");
    await userEvent.click(screen.getByRole("option", { name: /영역2/ }));
    expect(onLoadSelection).toHaveBeenCalledWith(saved[2]);
    // A loaded area becomes a chip.
    expect(screen.getByRole("button", { name: "target:영역2 · 3" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /전체 영역 10개/ }));
    await userEvent.click(screen.getByRole("button", { name: "영역0 선택 타겟 삭제" }));
    expect(onDeleteSelection).toHaveBeenCalledWith(saved[0]);
    expect(onLoadSelection).toHaveBeenCalledTimes(1);
  });
});
