import { describe, expect, it } from "vitest";

import {
  clampImagePlacement,
  fitImageDimensions,
  initialImagePlacement,
  moveImagePlacement,
  resizeImageFromHeight,
  resizeImageFromWidth,
  resizeImagePlacement,
} from "./imagePlacement";

const source = { width: 400, height: 200 };

describe("image placement transforms", () => {
  it("fits and centers portrait, landscape, square, and one-tile bounds", () => {
    expect(fitImageDimensions(source, 64, 64)).toEqual({ width: 64, height: 32 });
    expect(fitImageDimensions({ width: 200, height: 400 }, 64, 64)).toEqual({
      width: 32,
      height: 64,
    });
    expect(fitImageDimensions({ width: 1, height: 1 }, 256, 256)).toEqual({
      width: 256,
      height: 256,
    });
    expect(fitImageDimensions({ width: 1920, height: 1080 }, 1, 1)).toEqual({
      width: 1,
      height: 1,
    });
    expect(initialImagePlacement(source, 128, 128)).toEqual({
      x: 0,
      y: 32,
      width: 128,
      height: 64,
    });
  });

  it("snaps body movement to tiles and clamps every boundary", () => {
    const placement = { x: 10, y: 10, width: 8, height: 4 };
    expect(moveImagePlacement(placement, 1.49, -1.51, 32, 32)).toEqual({
      x: 11,
      y: 8,
      width: 8,
      height: 4,
    });
    expect(moveImagePlacement(placement, -100, 100, 32, 32)).toEqual({
      x: 0,
      y: 28,
      width: 8,
      height: 4,
    });
    expect(clampImagePlacement({ x: 99, y: 99, width: 99, height: 99 }, 16, 8)).toEqual({
      x: 0,
      y: 0,
      width: 16,
      height: 8,
    });
  });

  it("keeps source aspect while resizing from every corner", () => {
    const placement = { x: 10, y: 10, width: 8, height: 4 };
    expect(resizeImagePlacement(placement, "se", 20, 20, source, 32, 32)).toEqual({
      x: 10,
      y: 10,
      width: 10,
      height: 5,
    });
    expect(resizeImagePlacement(placement, "nw", 0, 0, source, 32, 32)).toEqual({
      x: 0,
      y: 5,
      width: 18,
      height: 9,
    });
    expect(resizeImagePlacement(placement, "ne", 40, -5, source, 32, 32)).toEqual({
      x: 10,
      y: 3,
      width: 22,
      height: 11,
    });
  });

  it("links numeric width and height edits and supports 1/8-tile keyboard deltas", () => {
    const placement = { x: 10, y: 10, width: 8, height: 4 };
    expect(resizeImageFromWidth(placement, 20, source, 64, 64)).toEqual({
      x: 10,
      y: 10,
      width: 20,
      height: 10,
    });
    expect(resizeImageFromHeight(placement, 12, source, 64, 64)).toEqual({
      x: 10,
      y: 10,
      width: 24,
      height: 12,
    });
    expect(moveImagePlacement(placement, -1, 0, 64, 64).x).toBe(9);
    expect(moveImagePlacement(placement, 8, 0, 64, 64).x).toBe(18);
  });
});
