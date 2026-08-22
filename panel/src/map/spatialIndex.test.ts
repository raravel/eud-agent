import { describe, expect, it } from "vitest";

import { MapSpatialIndex, type SpatialObject } from "./spatialIndex";

function object(id: string, z: number): SpatialObject {
  return {
    id,
    kind: "unit",
    bounds: { left: 4, top: 4, right: 7, bottom: 7 },
    z,
    item: {},
  };
}

describe("Map Agent structured hit testing", () => {
  it("cycles overlapping objects by structured CHK bounds and z-order", () => {
    const index = new MapSpatialIndex([
      object("unit-low", 20),
      object("unit-high", 40),
      object("unit-mid", 30),
    ]);
    expect(index.cycle(5, 5)?.id).toBe("unit-high");
    expect(index.cycle(5, 5, "unit-high")?.id).toBe("unit-mid");
    expect(index.cycle(5, 5, "unit-mid")?.id).toBe("unit-low");
    expect(index.cycle(5, 5, "unit-low")?.id).toBe("unit-high");
  });

  it("returns no hit outside object bounds", () => {
    const index = new MapSpatialIndex([object("unit", 1)]);
    expect(index.hit(2, 2)).toEqual([]);
    expect(index.cycle(2, 2)).toBeNull();
  });
});
