import type { TilePoint } from "./selectionMask";

export interface CanvasTransform {
  panX: number;
  panY: number;
  zoom: number;
}

export interface ScreenPoint {
  x: number;
  y: number;
}

export interface TileViewport {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export function mapToScreen(
  point: ScreenPoint,
  transform: CanvasTransform,
): ScreenPoint {
  return {
    x: point.x * transform.zoom + transform.panX,
    y: point.y * transform.zoom + transform.panY,
  };
}

export function screenToMap(
  point: ScreenPoint,
  transform: CanvasTransform,
): ScreenPoint {
  return {
    x: (point.x - transform.panX) / transform.zoom,
    y: (point.y - transform.panY) / transform.zoom,
  };
}

export function screenToTile(
  point: ScreenPoint,
  transform: CanvasTransform,
  width: number,
  height: number,
): TilePoint | null {
  const map = screenToMap(point, transform);
  const x = Math.floor(map.x / 32);
  const y = Math.floor(map.y / 32);
  if (x < 0 || y < 0 || x >= width || y >= height) return null;
  return { x, y };
}

export function fitMapTransform(options: {
  viewportWidth: number;
  viewportHeight: number;
  mapWidth: number;
  mapHeight: number;
  padding?: number;
}): CanvasTransform {
  const padding = options.padding ?? 24;
  const availableWidth = Math.max(1, options.viewportWidth - padding * 2);
  const availableHeight = Math.max(1, options.viewportHeight - padding * 2);
  const zoom = Math.min(
    1,
    availableWidth / (options.mapWidth * 32),
    availableHeight / (options.mapHeight * 32),
  );
  return {
    zoom,
    panX: (options.viewportWidth - options.mapWidth * 32 * zoom) / 2,
    panY: (options.viewportHeight - options.mapHeight * 32 * zoom) / 2,
  };
}

export function zoomAtPoint(
  transform: CanvasTransform,
  screen: ScreenPoint,
  factor: number,
): CanvasTransform {
  const before = screenToMap(screen, transform);
  const zoom = Math.max(0.125, Math.min(8, transform.zoom * factor));
  return {
    zoom,
    panX: screen.x - before.x * zoom,
    panY: screen.y - before.y * zoom,
  };
}

export function visibleTileBounds(options: {
  transform: CanvasTransform;
  viewportWidth: number;
  viewportHeight: number;
  mapWidth: number;
  mapHeight: number;
}): TileViewport {
  const topLeft = screenToMap({ x: 0, y: 0 }, options.transform);
  const bottomRight = screenToMap(
    { x: options.viewportWidth, y: options.viewportHeight },
    options.transform,
  );
  return {
    left: Math.max(0, Math.min(options.mapWidth, topLeft.x / 32)),
    top: Math.max(0, Math.min(options.mapHeight, topLeft.y / 32)),
    right: Math.max(0, Math.min(options.mapWidth, bottomRight.x / 32)),
    bottom: Math.max(0, Math.min(options.mapHeight, bottomRight.y / 32)),
  };
}

export function centerMapTransform(options: {
  transform: CanvasTransform;
  viewportWidth: number;
  viewportHeight: number;
  mapWidth: number;
  mapHeight: number;
  centerX: number;
  centerY: number;
}): CanvasTransform {
  const visibleWidth = options.viewportWidth / (32 * options.transform.zoom);
  const visibleHeight = options.viewportHeight / (32 * options.transform.zoom);
  const clampCenter = (requested: number, mapSize: number, visibleSize: number) =>
    visibleSize >= mapSize
      ? mapSize / 2
      : Math.max(visibleSize / 2, Math.min(mapSize - visibleSize / 2, requested));
  const centerX = clampCenter(options.centerX, options.mapWidth, visibleWidth);
  const centerY = clampCenter(options.centerY, options.mapHeight, visibleHeight);
  return {
    ...options.transform,
    panX: options.viewportWidth / 2 - centerX * 32 * options.transform.zoom,
    panY: options.viewportHeight / 2 - centerY * 32 * options.transform.zoom,
  };
}

export function visibleTileCrop(options: {
  transform: CanvasTransform;
  viewportWidth: number;
  viewportHeight: number;
  mapWidth: number;
  mapHeight: number;
  margin?: number;
}): { x: number; y: number; width: number; height: number } {
  const margin = options.margin ?? 2;
  const topLeft = screenToMap({ x: 0, y: 0 }, options.transform);
  const bottomRight = screenToMap(
    { x: options.viewportWidth, y: options.viewportHeight },
    options.transform,
  );
  const x = Math.min(
    Math.max(0, options.mapWidth - 1),
    Math.max(0, Math.floor(topLeft.x / 32) - margin),
  );
  const y = Math.min(
    Math.max(0, options.mapHeight - 1),
    Math.max(0, Math.floor(topLeft.y / 32) - margin),
  );
  const right = Math.max(
    x + 1,
    Math.min(options.mapWidth, Math.ceil(bottomRight.x / 32) + margin),
  );
  const bottom = Math.max(
    y + 1,
    Math.min(options.mapHeight, Math.ceil(bottomRight.y / 32) + margin),
  );
  return {
    x,
    y,
    width: Math.max(1, right - x),
    height: Math.max(1, bottom - y),
  };
}

export function bufferedTileCrop(options: {
  transform: CanvasTransform;
  viewportWidth: number;
  viewportHeight: number;
  mapWidth: number;
  mapHeight: number;
}): { x: number; y: number; width: number; height: number } {
  const visible = visibleTileCrop({ ...options, margin: 0 });
  const quantizedWidth = Math.ceil(visible.width / 4) * 4;
  const quantizedHeight = Math.ceil(visible.height / 4) * 4;
  const marginX = Math.max(4, Math.min(12, Math.ceil(quantizedWidth / 6)));
  const marginY = Math.max(4, Math.min(12, Math.ceil(quantizedHeight / 6)));
  const width = Math.min(options.mapWidth, quantizedWidth + marginX * 2);
  const height = Math.min(options.mapHeight, quantizedHeight + marginY * 2);
  const maxX = Math.max(0, options.mapWidth - width);
  const maxY = Math.max(0, options.mapHeight - height);
  const x = Math.max(
    0,
    Math.min(maxX, Math.floor((visible.x - marginX) / marginX) * marginX),
  );
  const y = Math.max(
    0,
    Math.min(maxY, Math.floor((visible.y - marginY) / marginY) * marginY),
  );
  return { x, y, width, height };
}
