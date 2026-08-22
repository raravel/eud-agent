import { describe, expect, it } from "vitest";

import {
  cellsToRows,
  combineCells,
  connectGridCells,
  freeMaskCells,
  rectangleCells,
  rowsToCells,
  selectionCellsForGesture,
} from "./selectionMask";

describe("Map Agent selection masks", () => {
  it("connects high-speed pointer samples without tile gaps", () => {
    const cells = connectGridCells({ x: 1, y: 1 }, { x: 17, y: 9 });
    expect(cells[0]).toEqual({ x: 1, y: 1 });
    expect(cells.at(-1)).toEqual({ x: 17, y: 9 });
    for (let index = 1; index < cells.length; index += 1) {
      expect(Math.abs(cells[index].x - cells[index - 1].x)).toBeLessThanOrEqual(1);
      expect(Math.abs(cells[index].y - cells[index - 1].y)).toBeLessThanOrEqual(1);
    }
  });

  it("ignores clicks and clears only when clicking outside the active selection", () => {
    const active = new Set(["1,1", "2,1"]);
    const gesture = (end: { x: number; y: number }) =>
      selectionCellsForGesture({
        baseCells: active,
        start: end,
        end,
        samples: [end],
        moved: false,
        shape: "rectangle",
        operation: "replace",
        width: 16,
        height: 16,
      });

    expect(gesture({ x: 1, y: 1 })).toEqual(active);
    expect(gesture({ x: 8, y: 8 })).toEqual(new Set());
    expect(
      selectionCellsForGesture({
        baseCells: new Set(),
        start: { x: 4, y: 4 },
        end: { x: 4, y: 4 },
        samples: [{ x: 4, y: 4 }],
        moved: false,
        shape: "rectangle",
        operation: "replace",
        width: 16,
        height: 16,
      }),
    ).toEqual(new Set());
  });

  it("fills a closed concave free mask by cell-center even/odd rule", () => {
    const cells = freeMaskCells(
      [
        { x: 1, y: 1 },
        { x: 6, y: 1 },
        { x: 6, y: 3 },
        { x: 3, y: 3 },
        { x: 3, y: 6 },
        { x: 1, y: 6 },
        { x: 1, y: 1 },
      ],
      10,
      10,
    );
    expect(cells.has("2,2")).toBe(true);
    expect(cells.has("2,5")).toBe(true);
    expect(cells.has("5,2")).toBe(true);
    expect(cells.has("5,5")).toBe(false);
  });

  it("supports holes, disjoint islands, add, subtract, and invert", () => {
    const outer = rectangleCells({ x: 1, y: 1 }, { x: 8, y: 8 }, 12, 12);
    const hole = rectangleCells({ x: 3, y: 3 }, { x: 6, y: 6 }, 12, 12);
    const withHole = combineCells(outer, hole, "subtract");
    expect(withHole.has("2,2")).toBe(true);
    expect(withHole.has("4,4")).toBe(false);

    const island = rectangleCells({ x: 10, y: 10 }, { x: 11, y: 11 }, 12, 12);
    const disjoint = combineCells(withHole, island, "add");
    expect(disjoint.has("10,10")).toBe(true);
    expect(disjoint.has("9,9")).toBe(false);

    const inverted = combineCells(disjoint, new Set(["2,2", "4,4"]), "invert");
    expect(inverted.has("2,2")).toBe(false);
    expect(inverted.has("4,4")).toBe(true);
    expect(rowsToCells(cellsToRows(inverted))).toEqual(inverted);
  });

  it("canonicalizes adjacent cells into sorted row spans", () => {
    const rows = cellsToRows(new Set(["4,2", "2,2", "3,2", "8,1", "6,1"]));
    expect(rows).toEqual([
      { y: 1, spans: [[6, 7], [8, 9]] },
      { y: 2, spans: [[2, 5]] },
    ]);
  });
});
