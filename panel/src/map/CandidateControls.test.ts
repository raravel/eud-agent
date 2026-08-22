import { describe, expect, it } from "vitest";

import { cumulativeDiff } from "./CandidateControls";

const emptyCount = { added: 0, removed: 0, moved: 0, changed: 0 };
const authority = {
  terrainCells: 0,
  units: emptyCount,
  buildings: emptyCount,
  doodads: emptyCount,
  sprites: emptyCount,
  locations: emptyCount,
  outsideTarget: 0,
  protected: 0,
  unsupportedSectionChanges: [],
};

describe("Map candidate cumulative diff", () => {
  it("counts original-to-current terrain bounds and every object change kind", () => {
    const diff = cumulativeDiff(
      {
        terrainRows: [
          { y: 2, spans: [[3, 6]] },
          { y: 4, spans: [[1, 2]] },
        ],
        markers: [
          { layer: "units", change: "moved", ordinal: 0, bounds: { left: 1, top: 1, right: 2, bottom: 2 } },
          { layer: "doodads", change: "changed", ordinal: 1, bounds: { left: 2, top: 2, right: 3, bottom: 3 } },
          { layer: "locations", change: "added", ordinal: 2, bounds: { left: 4, top: 4, right: 5, bottom: 5 } },
        ],
      },
      authority,
    );
    expect(diff.terrainCells).toBe(4);
    expect(diff.terrainBounds).toEqual({ left: 1, top: 2, right: 6, bottom: 5 });
    expect(diff.units.moved).toBe(1);
    expect(diff.doodads.changed).toBe(1);
    expect(diff.locations.added).toBe(1);
  });

  it("uses verified revision bounds until async detail rows arrive", () => {
    const diff = cumulativeDiff(
      { terrainRows: [], markers: [] },
      {
        ...authority,
        terrainCells: 2_032,
        terrainBounds: { left: 12, top: 68, right: 131, bottom: 128 },
      },
    );
    expect(diff.terrainCells).toBe(2_032);
    expect(diff.terrainBounds).toEqual({
      left: 12,
      top: 68,
      right: 131,
      bottom: 128,
    });
  });
});
