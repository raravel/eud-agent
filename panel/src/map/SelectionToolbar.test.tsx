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
});
