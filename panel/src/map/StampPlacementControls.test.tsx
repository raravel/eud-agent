import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { StampPlacementControls } from "./StampPlacementControls";
import type { SavedSelection, StampPlacementReport } from "./mapProtocol";

const selection: SavedSelection = {
  id: "selection-a",
  label: "영역 A",
  sourceRevision: "r1:hash",
  role: "target",
  layers: ["terrain", "units"],
  bounds: { left: 5, top: 5, right: 43, bottom: 35 },
  selectedCells: 1_140,
  rows: [{ y: 5, spans: [[5, 43]] }],
  snapshotHash: "snapshot-a",
};

function report(overrides: Partial<StampPlacementReport> = {}): StampPlacementReport {
  return {
    selectionId: selection.id,
    label: selection.label,
    width: 38,
    height: 30,
    layers: selection.layers,
    destinations: [{ x: 43, y: 5 }],
    terrainCellsPerDestination: 1_140,
    source: { units: 1, buildings: 0, doodads: 0, sprites: 0, locations: 0 },
    collisions: { units: 0, buildings: 0, doodads: 0, sprites: 0, locations: 0 },
    partialCollisions: { units: 0, buildings: 0, doodads: 0, sprites: 0, locations: 0 },
    outsideAuthorityCells: 0,
    protectedCells: 0,
    requiredLocationSlots: 0,
    availableLocationSlots: 10,
    ...overrides,
  };
}

const baseProps = {
  selection,
  destination: { x: 43, y: 5 },
  mapWidth: 256,
  mapHeight: 256,
  previewFresh: true,
  previewLoading: false,
  confirming: false,
  onDestination: vi.fn(),
  onConfirm: vi.fn(),
  onCancel: vi.fn(),
};

describe("Stamp placement collision choice", () => {
  it("places without policy friction when no destination object collides", async () => {
    const onConfirm = vi.fn();
    render(
      <StampPlacementControls
        {...baseProps}
        report={report()}
        onConfirm={onConfirm}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "후보에 배치" }));
    expect(onConfirm).toHaveBeenCalledWith("merge");
    expect(screen.queryByRole("button", { name: "교체" })).not.toBeInTheDocument();
  });

  it("requires merge, replace, or cancel and blocks unsafe partial replacement", async () => {
    const onConfirm = vi.fn();
    render(
      <StampPlacementControls
        {...baseProps}
        report={report({
          collisions: { units: 2, buildings: 0, doodads: 0, sprites: 0, locations: 1 },
          partialCollisions: { units: 1, buildings: 0, doodads: 0, sprites: 0, locations: 0 },
        })}
        onConfirm={onConfirm}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("병합, 교체 또는 취소");
    expect(screen.getByRole("button", { name: "교체" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "병합" }));
    expect(onConfirm).toHaveBeenCalledWith("merge");
  });
});
