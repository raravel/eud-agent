import type { MapImageDimensions, MapImagePlacement } from "./mapProtocol";

export type ImageResizeCorner = "nw" | "ne" | "sw" | "se";

export function fitImageDimensions(
  source: MapImageDimensions,
  maxWidth: number,
  maxHeight: number,
): { width: number; height: number } {
  if (
    !Number.isSafeInteger(source.width) ||
    !Number.isSafeInteger(source.height) ||
    source.width <= 0 ||
    source.height <= 0 ||
    !Number.isSafeInteger(maxWidth) ||
    !Number.isSafeInteger(maxHeight) ||
    maxWidth <= 0 ||
    maxHeight <= 0
  ) {
    throw new Error("사진 비율 계산에는 1 이상의 정수 크기가 필요합니다.");
  }
  if (maxWidth * source.height <= maxHeight * source.width) {
    return {
      width: maxWidth,
      height: Math.max(
        1,
        Math.min(
          maxHeight,
          Math.floor((source.height * maxWidth + source.width / 2) / source.width),
        ),
      ),
    };
  }
  return {
    width: Math.max(
      1,
      Math.min(
        maxWidth,
        Math.floor((source.width * maxHeight + source.height / 2) / source.height),
      ),
    ),
    height: maxHeight,
  };
}

export function initialImagePlacement(
  source: MapImageDimensions,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  const dimensions = fitImageDimensions(
    source,
    Math.min(256, mapWidth),
    Math.min(256, mapHeight),
  );
  return {
    x: Math.floor((mapWidth - dimensions.width) / 2),
    y: Math.floor((mapHeight - dimensions.height) / 2),
    ...dimensions,
  };
}

export function clampImagePlacement(
  placement: MapImagePlacement,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  const width = Math.max(1, Math.min(256, mapWidth, Math.round(placement.width)));
  const height = Math.max(1, Math.min(256, mapHeight, Math.round(placement.height)));
  return {
    x: Math.max(0, Math.min(mapWidth - width, Math.round(placement.x))),
    y: Math.max(0, Math.min(mapHeight - height, Math.round(placement.y))),
    width,
    height,
  };
}

export function moveImagePlacement(
  start: MapImagePlacement,
  deltaX: number,
  deltaY: number,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  return clampImagePlacement(
    {
      ...start,
      x: start.x + Math.round(deltaX),
      y: start.y + Math.round(deltaY),
    },
    mapWidth,
    mapHeight,
  );
}

export function resizeImagePlacement(
  start: MapImagePlacement,
  corner: ImageResizeCorner,
  pointerX: number,
  pointerY: number,
  source: MapImageDimensions,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  const anchorX = corner.endsWith("w") ? start.x + start.width : start.x;
  const anchorY = corner.startsWith("n") ? start.y + start.height : start.y;
  const snappedX = Math.round(pointerX);
  const snappedY = Math.round(pointerY);
  const availableWidth = corner.endsWith("w")
    ? Math.max(1, Math.min(anchorX, anchorX - snappedX))
    : Math.max(1, Math.min(mapWidth - anchorX, snappedX - anchorX));
  const availableHeight = corner.startsWith("n")
    ? Math.max(1, Math.min(anchorY, anchorY - snappedY))
    : Math.max(1, Math.min(mapHeight - anchorY, snappedY - anchorY));
  const dimensions = fitImageDimensions(source, availableWidth, availableHeight);
  return {
    x: corner.endsWith("w") ? anchorX - dimensions.width : anchorX,
    y: corner.startsWith("n") ? anchorY - dimensions.height : anchorY,
    ...dimensions,
  };
}

export function resizeImageFromWidth(
  placement: MapImagePlacement,
  width: number,
  source: MapImageDimensions,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  const requestedWidth = Math.max(1, Math.min(256, mapWidth, Math.round(width)));
  const requestedHeight = Math.max(
    1,
    Math.round((requestedWidth * source.height) / source.width),
  );
  const dimensions = fitImageDimensions(
    source,
    requestedWidth,
    Math.min(256, mapHeight, requestedHeight),
  );
  return clampImagePlacement({ ...placement, ...dimensions }, mapWidth, mapHeight);
}

export function resizeImageFromHeight(
  placement: MapImagePlacement,
  height: number,
  source: MapImageDimensions,
  mapWidth: number,
  mapHeight: number,
): MapImagePlacement {
  const requestedHeight = Math.max(1, Math.min(256, mapHeight, Math.round(height)));
  const requestedWidth = Math.max(
    1,
    Math.round((requestedHeight * source.width) / source.height),
  );
  const dimensions = fitImageDimensions(
    source,
    Math.min(256, mapWidth, requestedWidth),
    requestedHeight,
  );
  return clampImagePlacement({ ...placement, ...dimensions }, mapWidth, mapHeight);
}

export function sameImagePlacement(
  left: MapImagePlacement,
  right: MapImagePlacement,
): boolean {
  return (
    left.x === right.x &&
    left.y === right.y &&
    left.width === right.width &&
    left.height === right.height
  );
}
