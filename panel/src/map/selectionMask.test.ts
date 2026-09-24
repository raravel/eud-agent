import { describe, expect, it } from "vitest";

import {
  cellsToRows,
  combineCells,
  connectGridCells,
  freeMaskCells,
  rectangleCells,
  rowSpanOutline,
  rowsToCells,
  selectionCellsForGesture,
} from "./selectionMask";

const sortSegments = (segments: ReturnType<typeof rowSpanOutline>) =>
  segments
    .map(({ x0, y0, x1, y1 }) => `${x0},${y0}-${x1},${y1}`)
    .sort();

describe("Map Agent selection masks", () => {
  it("outlines a rectangle with only its four outer edges", () => {
    const rows = cellsToRows(rectangleCells({ x: 1, y: 1 }, { x: 3, y: 2 }, 16, 16));
    expect(sortSegments(rowSpanOutline(rows))).toEqual(
      sortSegments([
        { x0: 1, y0: 1, x1: 1, y1: 2 },
        { x0: 1, y0: 2, x1: 1, y1: 3 },
        { x0: 4, y0: 1, x1: 4, y1: 2 },
        { x0: 4, y0: 2, x1: 4, y1: 3 },
        { x0: 1, y0: 1, x1: 4, y1: 1 },
        { x0: 1, y0: 3, x1: 4, y1: 3 },
      ]),
    );
  });

  it("draws no edge between vertically or horizontally adjacent selected tiles", () => {
    // L shape: (0,0) (1,0) (0,1)
    const rows = cellsToRows(new Set(["0,0", "1,0", "0,1"]));
    const segments = sortSegments(rowSpanOutline(rows));
    expect(segments).not.toContain("1,0-1,1"); // between (0,0) and (1,0)
    expect(segments).not.toContain("0,1-1,1"); // between (0,0) and (0,1)
    expect(segments).toEqual(
      sortSegments([
        { x0: 0, y0: 0, x1: 2, y1: 0 }, // top of both row-0 tiles, one line
        { x0: 0, y0: 0, x1: 0, y1: 1 },
        { x0: 2, y0: 0, x1: 2, y1: 1 },
        { x0: 1, y0: 1, x1: 2, y1: 1 }, // under (1,0), where (1,1) is unselected
        { x0: 0, y0: 1, x1: 0, y1: 2 },
        { x0: 1, y0: 1, x1: 1, y1: 2 }, // right of (0,1)
        { x0: 0, y0: 2, x1: 1, y1: 2 },
      ]),
    );
  });

  it("outlines disjoint spans on one row separately", () => {
    const rows = cellsToRows(new Set(["0,0", "2,0"]));
    expect(rowSpanOutline(rows)).toHaveLength(8);
  });

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
