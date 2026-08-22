import { describe, expect, it } from "vitest";

import {
  bufferedTileCrop,
  centerMapTransform,
  fitMapTransform,
  mapToScreen,
  screenToMap,
  visibleTileCrop,
  visibleTileBounds,
  zoomAtPoint,
} from "./canvasTransform";

describe("Map Agent canvas transforms", () => {
  it("round-trips map and screen coordinates", () => {
    const transform = { panX: 137.25, panY: -82.5, zoom: 1.75 };
    const map = { x: 934.5, y: 217.25 };
    const roundTrip = screenToMap(mapToScreen(map, transform), transform);
    expect(roundTrip.x).toBeCloseTo(map.x, 10);
    expect(roundTrip.y).toBeCloseTo(map.y, 10);
  });

  it("keeps the map point under the cursor stable while zooming", () => {
    const transform = { panX: 80, panY: 45, zoom: 0.5 };
    const cursor = { x: 420, y: 280 };
    const before = screenToMap(cursor, transform);
    const next = zoomAtPoint(transform, cursor, 1.2);
    const after = screenToMap(cursor, next);
    expect(after.x).toBeCloseTo(before.x, 10);
    expect(after.y).toBeCloseTo(before.y, 10);
  });

  it("fits and bounds the requested viewport crop", () => {
    const transform = fitMapTransform({
      viewportWidth: 1280,
      viewportHeight: 800,
      mapWidth: 128,
      mapHeight: 128,
    });
    const crop = visibleTileCrop({
      transform,
      viewportWidth: 1280,
      viewportHeight: 800,
      mapWidth: 128,
      mapHeight: 128,
    });
    expect(crop.x).toBeGreaterThanOrEqual(0);
    expect(crop.y).toBeGreaterThanOrEqual(0);
    expect(crop.x + crop.width).toBeLessThanOrEqual(128);
    expect(crop.y + crop.height).toBeLessThanOrEqual(128);
  });

  it("keeps a buffered crop stable across small pans while covering the viewport", () => {
    const options = {
      viewportWidth: 640,
      viewportHeight: 480,
      mapWidth: 256,
      mapHeight: 256,
    };
    const firstTransform = { panX: -40 * 32, panY: -30 * 32, zoom: 1 };
    const secondTransform = { ...firstTransform, panX: firstTransform.panX - 32 };
    const first = bufferedTileCrop({ ...options, transform: firstTransform });
    const second = bufferedTileCrop({ ...options, transform: secondTransform });
    const visible = visibleTileCrop({ ...options, transform: secondTransform, margin: 0 });

    expect(second).toEqual(first);
    expect(second.x).toBeLessThanOrEqual(visible.x);
    expect(second.y).toBeLessThanOrEqual(visible.y);
    expect(second.x + second.width).toBeGreaterThanOrEqual(visible.x + visible.width);
    expect(second.y + second.height).toBeGreaterThanOrEqual(visible.y + visible.height);
  });

  it("clamps a viewport panned completely beyond the map to a valid edge crop", () => {
    const crop = visibleTileCrop({
      transform: { panX: -100_000, panY: -100_000, zoom: 1 },
      viewportWidth: 1280,
      viewportHeight: 800,
      mapWidth: 64,
      mapHeight: 32,
    });
    expect(crop).toEqual({ x: 63, y: 31, width: 1, height: 1 });
  });

  it("reports the exact visible tile viewport for minimap framing", () => {
    const viewport = visibleTileBounds({
      transform: { panX: -32 * 10, panY: -32 * 20, zoom: 1 },
      viewportWidth: 640,
      viewportHeight: 320,
      mapWidth: 128,
      mapHeight: 128,
    });
    expect(viewport).toEqual({ left: 10, top: 20, right: 30, bottom: 30 });
  });

  it("centers minimap navigation while clamping map edges", () => {
    const centered = centerMapTransform({
      transform: { panX: 0, panY: 0, zoom: 1 },
      viewportWidth: 640,
      viewportHeight: 320,
      mapWidth: 128,
      mapHeight: 128,
      centerX: 64,
      centerY: 64,
    });
    expect(
      visibleTileBounds({
        transform: centered,
        viewportWidth: 640,
        viewportHeight: 320,
        mapWidth: 128,
        mapHeight: 128,
      }),
    ).toEqual({ left: 54, top: 59, right: 74, bottom: 69 });

    const edge = centerMapTransform({
      transform: centered,
      viewportWidth: 640,
      viewportHeight: 320,
      mapWidth: 128,
      mapHeight: 128,
      centerX: 0,
      centerY: 0,
    });
    expect(
      visibleTileBounds({
        transform: edge,
        viewportWidth: 640,
        viewportHeight: 320,
        mapWidth: 128,
        mapHeight: 128,
      }),
    ).toEqual({ left: 0, top: 0, right: 20, bottom: 10 });
  });
});
