import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import {
  bufferedTileCrop,
  centerMapTransform,
  fitMapTransform,
  mapToScreen,
  screenToTile,
  screenToMap,
  visibleTileBounds,
  zoomAtPoint,
  type CanvasTransform,
  type TileViewport,
} from "./canvasTransform";
import {
  cellsToRows,
  selectionCellsForGesture,
  type TilePoint,
} from "./selectionMask";
import {
  moveImagePlacement,
  resizeImagePlacement,
  type ImageResizeCorner,
} from "./imagePlacement";
import {
  MapSpatialIndex,
  spatialObjectsFromPages,
  type SpatialObject,
} from "./spatialIndex";
import {
  type MapLayer,
  type MapObjectItem,
  type MapDiffMarker,
  type RowSpan,
  type MapRenderSource,
  type MapView,
  type SelectionMask,
  type SelectionOperation,
  type SelectionShape,
  type TileRect,
  type MapImageDimensions,
  type MapImagePlacement,
  type StampDestination,
} from "./mapProtocol";

interface CropImage {
  crop: { x: number; y: number; width: number; height: number };
  scale: number;
  bitmap: ImageBitmap;
}

export interface MapCanvasProps {
  renderSource: MapRenderSource;
  ariaLabel: string;
  width: number;
  height: number;
  view: MapView;
  layers: MapLayer[];
  selections: SelectionMask[];
  activeCells: Set<string>;
  selectionShape: SelectionShape;
  selectionOperation: SelectionOperation;
  interactionMode: "select" | "inspect" | "pan";
  objects: MapObjectItem[];
  diffRows: RowSpan[];
  diffMarkers: MapDiffMarker[];
  highlightedObjectId?: string;
  focusTarget?: { bounds?: TileRect; objectId?: string; sequence: number };
  viewportTarget?: { x: number; y: number; sequence: number };
  imagePlacement?: {
    placement: MapImagePlacement;
    sourceDimensions: MapImageDimensions;
    bitmap: ImageBitmap | null;
    previewMode: "original" | "result";
    canConfirm: boolean;
  };
  stampPlacement?: {
    destination: StampDestination;
    sourceBounds: TileRect;
    rows: RowSpan[];
    canConfirm: boolean;
  };
  highlightedSelectionId?: string;
  onActiveCells(cells: Set<string>): void;
  onCursor(tile: TilePoint | null): void;
  onObjectSelect(object: SpatialObject | null): void;
  onZoom(zoom: number): void;
  onSelectionAnchor(anchor: { x: number; y: number } | null): void;
  onViewportChange?(viewport: TileViewport): void;
  onImagePlacement?(placement: MapImagePlacement, settled: boolean): void;
  onImageConfirm?(): void;
  onImageCancel?(): void;
  onStampPlacement?(destination: StampDestination, settled: boolean): void;
  onStampConfirm?(): void;
  onStampCancel?(): void;
}

const selectionStyle: Record<SelectionMask["role"], { fill: string; stroke: string; dash: number[] }> = {
  target: { fill: "rgba(16,185,129,.14)", stroke: "#34d399", dash: [] },
  reference: { fill: "rgba(59,130,246,.11)", stroke: "#60a5fa", dash: [6, 4] },
  protect: { fill: "rgba(244,63,94,.13)", stroke: "#fb7185", dash: [3, 3] },
  anchor: { fill: "rgba(245,158,11,.12)", stroke: "#fbbf24", dash: [10, 4, 2, 4] },
};

function nativeScaleForZoom(zoom: number): number {
  if (zoom >= 0.75) return 1;
  if (zoom >= 0.375) return 2;
  if (zoom >= 0.1875) return 4;
  return 8;
}

function objectLayer(kind: SpatialObject["kind"]): MapLayer {
  if (kind === "unit") return "units";
  if (kind === "building") return "buildings";
  if (kind === "doodad") return "doodads";
  if (kind === "sprite") return "sprites";
  return "locations";
}

export function MapCanvas({
  renderSource,
  ariaLabel,
  width,
  height,
  view,
  layers,
  selections,
  activeCells,
  selectionShape,
  selectionOperation,
  interactionMode,
  objects,
  diffRows,
  diffMarkers,
  highlightedObjectId,
  focusTarget,
  viewportTarget,
  highlightedSelectionId,
  imagePlacement,
  stampPlacement,
  onActiveCells,
  onCursor,
  onObjectSelect,
  onZoom,
  onSelectionAnchor,
  onViewportChange,
  onImagePlacement,
  onImageConfirm,
  onImageCancel,
  onStampPlacement,
  onStampConfirm,
  onStampCancel,
}: MapCanvasProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const cacheRef = useRef(new Map<string, CropImage>());
  const dragRef = useRef<{
    pointerId: number;
    startScreen: { x: number; y: number };
    startTile: TilePoint;
    samples: TilePoint[];
    transform: CanvasTransform;
    panning: boolean;
    moved: boolean;
    baseCells: Set<string>;
    shape: SelectionShape;
    operation: SelectionOperation;
  } | null>(null);
  const imageDragRef = useRef<{
    pointerId: number;
    kind: "move" | "resize";
    corner?: ImageResizeCorner;
    startMap: { x: number; y: number };
    startPlacement: MapImagePlacement;
  } | null>(null);
  const imageFrameRef = useRef<number | null>(null);
  const pendingImagePlacementRef = useRef<MapImagePlacement | null>(null);
  const fitDoneRef = useRef(false);
  const lastHitRef = useRef<{ tile: string; id?: string }>({ tile: "" });
  const frameRef = useRef<number | null>(null);
  const cursorFrameRef = useRef<number | null>(null);
  const pendingCursorRef = useRef<TilePoint | null>(null);
  const panFrameRef = useRef<number | null>(null);
  const pendingPanRef = useRef<CanvasTransform | null>(null);
  const wheelFrameRef = useRef<number | null>(null);
  const pendingWheelRef = useRef<CanvasTransform | null>(null);
  const previewFrameRef = useRef<number | null>(null);
  const pendingPreviewTileRef = useRef<TilePoint | null>(null);
  const lastRenderSourceKeyRef = useRef("");
  const [size, setSize] = useState({ width: 1, height: 1, ratio: 1 });
  const [transform, setTransform] = useState<CanvasTransform>({
    panX: 0,
    panY: 0,
    zoom: 0.25,
  });
  const transformRef = useRef(transform);
  transformRef.current = transform;
  const [cropImage, setCropImage] = useState<CropImage | null>(null);
  const [loading, setLoading] = useState(false);
  const [previewCells, setPreviewCells] = useState<Set<string> | null>(null);
  const [renderError, setRenderError] = useState("");
  const [showGrid, setShowGrid] = useState(true);
  const spatialObjects = useMemo(() => spatialObjectsFromPages(objects), [objects]);
  const visibleSpatialObjects = useMemo(
    () => spatialObjects.filter((object) => layers.includes(objectLayer(object.kind))),
    [layers, spatialObjects],
  );
  const spatialIndex = useMemo(
    () => new MapSpatialIndex(visibleSpatialObjects),
    [visibleSpatialObjects],
  );
  const displayedCells = previewCells ?? activeCells;
  const activeRows = useMemo(() => cellsToRows(displayedCells), [displayedCells]);
  const committedRows = useMemo(() => cellsToRows(activeCells), [activeCells]);
  const activeBounds = useMemo(() => {
    if (committedRows.length === 0) return null;
    let left = width;
    let right = 0;
    for (const row of committedRows) {
      for (const [spanLeft, spanRight] of row.spans) {
        left = Math.min(left, spanLeft);
        right = Math.max(right, spanRight);
      }
    }
    return {
      left,
      top: committedRows[0].y,
      right,
      bottom: committedRows[committedRows.length - 1].y + 1,
    };
  }, [committedRows, width]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const observer = new ResizeObserver(([entry]) => {
      const ratio = window.devicePixelRatio || 1;
      setSize({
        width: Math.max(1, Math.floor(entry.contentRect.width)),
        height: Math.max(1, Math.floor(entry.contentRect.height)),
        ratio,
      });
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    fitDoneRef.current = false;
    setCropImage(null);
    lastHitRef.current = { tile: "" };
    for (const image of cacheRef.current.values()) image.bitmap.close();
    cacheRef.current.clear();
  }, [height, renderSource.key, width]);

  useEffect(() => {
    if (fitDoneRef.current || size.width <= 1 || size.height <= 1) return;
    const fitted = fitMapTransform({
      viewportWidth: size.width,
      viewportHeight: size.height,
      mapWidth: width,
      mapHeight: height,
    });
    fitDoneRef.current = true;
    setTransform(fitted);
    onZoom(fitted.zoom);
  }, [height, onZoom, size.height, size.width, width]);
  useEffect(() => {
    if (!focusTarget) return;
    const bounds =
      focusTarget.bounds ??
      visibleSpatialObjects.find((item) => item.id === focusTarget.objectId)?.bounds;
    if (!bounds) return;
    const centerX = ((bounds.left + bounds.right) / 2) * 32;
    const centerY = ((bounds.top + bounds.bottom) / 2) * 32;
    setTransform((current) =>
      centerMapTransform({
        transform: current,
        viewportWidth: size.width,
        viewportHeight: size.height,
        mapWidth: width,
        mapHeight: height,
        centerX: centerX / 32,
        centerY: centerY / 32,
      }),
    );
  }, [focusTarget, height, size.height, size.width, visibleSpatialObjects, width]);
  useEffect(() => {
    if (!viewportTarget || size.width <= 1 || size.height <= 1) return;
    const next = centerMapTransform({
      transform: transformRef.current,
      viewportWidth: size.width,
      viewportHeight: size.height,
      mapWidth: width,
      mapHeight: height,
      centerX: viewportTarget.x,
      centerY: viewportTarget.y,
    });
    transformRef.current = next;
    setTransform(next);
  }, [height, size.height, size.width, viewportTarget, width]);

  useEffect(() => {
    if (!onViewportChange || size.width <= 1 || size.height <= 1) return;
    onViewportChange(
      visibleTileBounds({
        transform,
        viewportWidth: size.width,
        viewportHeight: size.height,
        mapWidth: width,
        mapHeight: height,
      }),
    );
  }, [
    height,
    onViewportChange,
    size.height,
    size.width,
    transform,
    width,
  ]);
  useEffect(() => {
    if (!activeBounds) {
      onSelectionAnchor(null);
      return;
    }
    const screen = mapToScreen(
      {
        x: ((activeBounds.left + activeBounds.right) / 2) * 32,
        y: activeBounds.top * 32,
      },
      transform,
    );
    const safeCenter =
      size.width < 560
        ? size.width / 2
        : Math.max(260, Math.min(size.width - 260, screen.x));
    onSelectionAnchor({
      x: safeCenter,
      y: Math.max(72, Math.min(size.height - 8, screen.y - 8)),
    });
  }, [activeBounds, onSelectionAnchor, size.height, size.width, transform]);




  const crop = useMemo(
    () =>
      bufferedTileCrop({
        transform,
        viewportWidth: size.width,
        viewportHeight: size.height,
        mapWidth: width,
        mapHeight: height,
      }),
    [height, size.height, size.width, transform, width],
  );
  const nativeScale = nativeScaleForZoom(transform.zoom);
  const layerKey = layers.slice().sort().join(",");
  const renderSourceKey = `${renderSource.key}|${layerKey}`;

  useEffect(() => {
    let disposed = false;
    const sourceChanged = lastRenderSourceKeyRef.current !== renderSourceKey;
    lastRenderSourceKeyRef.current = renderSourceKey;
    const key = `${renderSourceKey}|${crop.x},${crop.y},${crop.width},${crop.height}|${nativeScale}`;
    const cache = cacheRef.current;
    const cached = cache.get(key);
    if (cached) {
      cache.delete(key);
      cache.set(key, cached);
      setCropImage(cached);
      setRenderError("");
      setLoading(false);
      return;
    }
    const timer = window.setTimeout(
      () => {
        setLoading(true);
        void renderSource
          .render({
            ...crop,
            scale: nativeScale,
            layers,
          })
          .then((blob) => createImageBitmap(blob))
          .then((bitmap) => {
            if (disposed) {
              bitmap.close();
              return;
            }
            const image = { crop, scale: nativeScale, bitmap };
            cache.set(key, image);
            let cachedPixels = 0;
            for (const entry of cache.values()) {
              cachedPixels += entry.bitmap.width * entry.bitmap.height;
            }
            while (cache.size > 12 || cachedPixels > 24_000_000) {
              const oldest = cache.keys().next().value as string | undefined;
              if (!oldest || (oldest === key && cache.size === 1)) break;
              const evicted = cache.get(oldest);
              if (evicted) {
                cachedPixels -= evicted.bitmap.width * evicted.bitmap.height;
                evicted.bitmap.close();
              }
              cache.delete(oldest);
            }
            setCropImage(image);
            setRenderError("");
          })
          .catch((error) => {
            if (!disposed) setRenderError(String(error));
          })
          .finally(() => {
            if (!disposed) setLoading(false);
          });
      },
      sourceChanged ? 0 : 90,
    );
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [crop, layers, nativeScale, renderSource, renderSourceKey]);

  useEffect(
    () => () => {
      for (const image of cacheRef.current.values()) image.bitmap.close();
      cacheRef.current.clear();
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
      if (cursorFrameRef.current !== null) cancelAnimationFrame(cursorFrameRef.current);
      if (panFrameRef.current !== null) cancelAnimationFrame(panFrameRef.current);
      if (wheelFrameRef.current !== null) cancelAnimationFrame(wheelFrameRef.current);
      if (previewFrameRef.current !== null) cancelAnimationFrame(previewFrameRef.current);
      if (imageFrameRef.current !== null) cancelAnimationFrame(imageFrameRef.current);
    },
    [],
  );

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const pixelWidth = Math.max(1, Math.floor(size.width * size.ratio));
    const pixelHeight = Math.max(1, Math.floor(size.height * size.ratio));
    if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
      canvas.width = pixelWidth;
      canvas.height = pixelHeight;
    }
    const context = canvas.getContext("2d");
    if (!context) return;
    context.setTransform(size.ratio, 0, 0, size.ratio, 0, 0);
    context.clearRect(0, 0, size.width, size.height);
    context.fillStyle = "#071019";
    context.fillRect(0, 0, size.width, size.height);

    if (cropImage) {
      const topLeft = mapToScreen(
        { x: cropImage.crop.x * 32, y: cropImage.crop.y * 32 },
        transform,
      );
      context.imageSmoothingEnabled = false;
      context.drawImage(
        cropImage.bitmap,
        topLeft.x,
        topLeft.y,
        cropImage.crop.width * 32 * transform.zoom,
        cropImage.crop.height * 32 * transform.zoom,
      );
    }

    const tileSize = 32 * transform.zoom;
    if (showGrid && tileSize >= 8) {
      const topLeft = mapToScreen({ x: 0, y: 0 }, transform);
      context.beginPath();
      context.strokeStyle = tileSize >= 20 ? "rgba(226,232,240,.22)" : "rgba(226,232,240,.1)";
      context.lineWidth = 1;
      for (let x = 0; x <= width; x += 1) {
        const screenX = topLeft.x + x * tileSize;
        context.moveTo(screenX, topLeft.y);
        context.lineTo(screenX, topLeft.y + height * tileSize);
      }
      for (let y = 0; y <= height; y += 1) {
        const screenY = topLeft.y + y * tileSize;
        context.moveTo(topLeft.x, screenY);
        context.lineTo(topLeft.x + width * tileSize, screenY);
      }
      context.stroke();
    }

    if (view === "diff") {
      context.fillStyle = "rgba(236,72,153,.35)";
      context.strokeStyle = "#f472b6";
      context.lineWidth = 1.5;
      for (const row of layers.includes("terrain") ? diffRows : []) {
        for (const [left, right] of row.spans) {
          const screen = mapToScreen({ x: left * 32, y: row.y * 32 }, transform);
          const spanWidth = (right - left) * tileSize;
          context.fillRect(screen.x, screen.y, spanWidth, tileSize);
          context.strokeRect(screen.x, screen.y, spanWidth, tileSize);
        }
      }
      const markerColors: Record<MapDiffMarker["change"], string> = {
        added: "#4ade80",
        removed: "#fb7185",
        moved: "#fbbf24",
        changed: "#c084fc",
      };
      context.font = "600 11px system-ui";
      for (const marker of diffMarkers.filter((item) => layers.includes(item.layer))) {
        const screen = mapToScreen(
          { x: marker.bounds.left * 32, y: marker.bounds.top * 32 },
          transform,
        );
        context.strokeStyle = markerColors[marker.change];
        context.lineWidth = 3;
        context.setLineDash(marker.change === "removed" ? [4, 3] : []);
        context.strokeRect(
          screen.x,
          screen.y,
          (marker.bounds.right - marker.bounds.left) * tileSize,
          (marker.bounds.bottom - marker.bounds.top) * tileSize,
        );
        context.fillStyle = markerColors[marker.change];
        context.fillText(
          `${marker.change} · ${marker.layer}`,
          screen.x + 3,
          screen.y - 4,
        );
      }
      context.setLineDash([]);
    }

    if (imagePlacement) {
      const imageScreen = mapToScreen(
        {
          x: imagePlacement.placement.x * 32,
          y: imagePlacement.placement.y * 32,
        },
        transform,
      );
      const imageWidth = imagePlacement.placement.width * tileSize;
      const imageHeight = imagePlacement.placement.height * tileSize;
      context.save();
      context.globalAlpha = imagePlacement.previewMode === "original" ? 0.72 : 1;
      context.imageSmoothingEnabled = imagePlacement.previewMode === "original";
      if (imagePlacement.bitmap) {
        context.drawImage(
          imagePlacement.bitmap,
          imageScreen.x,
          imageScreen.y,
          imageWidth,
          imageHeight,
        );
      } else {
        context.fillStyle = "rgba(8,145,178,.2)";
        context.fillRect(imageScreen.x, imageScreen.y, imageWidth, imageHeight);
      }
      context.restore();
    }
    if (stampPlacement) {
      const offsetX = stampPlacement.destination.x - stampPlacement.sourceBounds.left;
      const offsetY = stampPlacement.destination.y - stampPlacement.sourceBounds.top;
      context.save();
      context.fillStyle = "rgba(34,211,238,.2)";
      context.strokeStyle = stampPlacement.canConfirm ? "#22d3ee" : "#f59e0b";
      context.lineWidth = 2;
      context.setLineDash(stampPlacement.canConfirm ? [] : [6, 4]);
      for (const row of stampPlacement.rows) {
        for (const [left, right] of row.spans) {
          const screen = mapToScreen(
            { x: (left + offsetX) * 32, y: (row.y + offsetY) * 32 },
            transform,
          );
          const spanWidth = (right - left) * tileSize;
          context.fillRect(screen.x, screen.y, spanWidth, tileSize);
          context.strokeRect(screen.x, screen.y, spanWidth, tileSize);
        }
      }
      context.restore();
    }


    for (const selection of selections) {
      const style = selectionStyle[selection.role];
      context.fillStyle = style.fill;
      context.strokeStyle = style.stroke;
      context.lineWidth =
        selection.id === highlightedSelectionId ? 3 : selection.role === "protect" ? 2 : 1.5;
      context.setLineDash(style.dash);
      for (const row of selection.rows) {
        for (const [left, right] of row.spans) {
          const screen = mapToScreen({ x: left * 32, y: row.y * 32 }, transform);
          const spanWidth = (right - left) * tileSize;
          context.fillRect(screen.x, screen.y, spanWidth, tileSize);
          context.strokeRect(screen.x, screen.y, spanWidth, tileSize);
        }
      }
      context.setLineDash([]);
      const labelPosition = mapToScreen(
        { x: selection.bounds.left * 32, y: selection.bounds.top * 32 },
        transform,
      );
      context.fillStyle = style.stroke;
      context.font = "600 12px system-ui";
      context.fillText(`${selection.role.toUpperCase()} · ${selection.label}`, labelPosition.x + 4, labelPosition.y - 6);
    }

    context.fillStyle = "rgba(250,204,21,.28)";
    context.strokeStyle = "#fde047";
    context.lineWidth = 1.5;
    for (const row of activeRows) {
      if (row.y < crop.y || row.y >= crop.y + crop.height) continue;
      for (const [left, right] of row.spans) {
        const visibleLeft = Math.max(left, crop.x);
        const visibleRight = Math.min(right, crop.x + crop.width);
        if (visibleLeft >= visibleRight) continue;
        const screen = mapToScreen(
          { x: visibleLeft * 32, y: row.y * 32 },
          transform,
        );
        const spanWidth = (visibleRight - visibleLeft) * tileSize;
        context.fillRect(screen.x, screen.y, spanWidth, tileSize);
        if (tileSize >= 5) context.strokeRect(screen.x, screen.y, spanWidth, tileSize);
      }
    }

    for (const object of visibleSpatialObjects) {
      if (object.kind !== "location" && object.id !== highlightedObjectId) continue;
      const screen = mapToScreen(
        { x: object.bounds.left * 32, y: object.bounds.top * 32 },
        transform,
      );
      context.strokeStyle = object.kind === "location" ? "rgba(34,211,238,.75)" : "#f8fafc";
      context.setLineDash(object.kind === "location" ? [8, 4] : [3, 2]);
      context.lineWidth = object.id === highlightedObjectId ? 3 : 1.5;
      context.strokeRect(
        screen.x,
        screen.y,
        (object.bounds.right - object.bounds.left) * tileSize,
        (object.bounds.bottom - object.bounds.top) * tileSize,
      );
      if (object.kind === "location" && object.item.location) {
        context.setLineDash([]);
        context.fillStyle = "rgba(165,243,252,.95)";
        context.font = "600 11px system-ui";
        context.fillText(
          `#${object.item.location.id} ${object.item.location.name || "이름 없음"}`,
          screen.x + 3,
          screen.y + 14,
        );
      }
    }
    context.setLineDash([]);
    if (imagePlacement) {
      const imageScreen = mapToScreen(
        {
          x: imagePlacement.placement.x * 32,
          y: imagePlacement.placement.y * 32,
        },
        transform,
      );
      const imageWidth = imagePlacement.placement.width * tileSize;
      const imageHeight = imagePlacement.placement.height * tileSize;
      context.strokeStyle = imagePlacement.canConfirm ? "#22d3ee" : "#f59e0b";
      context.lineWidth = 2;
      context.setLineDash(imagePlacement.canConfirm ? [] : [6, 4]);
      context.strokeRect(imageScreen.x, imageScreen.y, imageWidth, imageHeight);
      context.setLineDash([]);
      const handleSize = 12;
      context.fillStyle = "#e0f2fe";
      context.strokeStyle = "#0e7490";
      for (const [x, y] of [
        [imageScreen.x, imageScreen.y],
        [imageScreen.x + imageWidth, imageScreen.y],
        [imageScreen.x, imageScreen.y + imageHeight],
        [imageScreen.x + imageWidth, imageScreen.y + imageHeight],
      ]) {
        context.fillRect(
          x - handleSize / 2,
          y - handleSize / 2,
          handleSize,
          handleSize,
        );
        context.strokeRect(
          x - handleSize / 2,
          y - handleSize / 2,
          handleSize,
          handleSize,
        );
      }
    }
  }, [
    activeRows,
    crop,
    cropImage,
    diffMarkers,
    diffRows,
    height,
    highlightedObjectId,
    highlightedSelectionId,
    imagePlacement,
    layers,
    selections,
    stampPlacement,
    showGrid,
    size,
    transform,
    view,
    visibleSpatialObjects,
    width,
  ]);

  useEffect(() => {
    if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
    frameRef.current = requestAnimationFrame(draw);
    return () => {
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
    };
  }, [draw]);

  const screenPoint = useCallback((event: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }, []);

  const queueCursor = useCallback(
    (tile: TilePoint | null) => {
      pendingCursorRef.current = tile;
      if (cursorFrameRef.current !== null) return;
      cursorFrameRef.current = requestAnimationFrame(() => {
        cursorFrameRef.current = null;
        onCursor(pendingCursorRef.current);
      });
    },
    [onCursor],
  );

  const queueSelectionPreview = useCallback(
    (tile: TilePoint) => {
      pendingPreviewTileRef.current = tile;
      if (previewFrameRef.current !== null) return;
      previewFrameRef.current = requestAnimationFrame(() => {
        previewFrameRef.current = null;
        const drag = dragRef.current;
        const end = pendingPreviewTileRef.current;
        pendingPreviewTileRef.current = null;
        if (!drag || !end || !drag.moved || drag.panning) return;
        setPreviewCells(
          selectionCellsForGesture({
            baseCells: drag.baseCells,
            start: drag.startTile,
            end,
            samples: drag.samples,
            moved: true,
            shape: drag.shape,
            operation: drag.operation,
            width,
            height,
          }),
        );
      });
    },
    [height, width],
  );

  const imagePlacementForPointer = useCallback(
    (
      screen: { x: number; y: number },
      drag: NonNullable<typeof imageDragRef.current>,
    ): MapImagePlacement => {
      const map = screenToMap(screen, transformRef.current);
      const tileX = map.x / 32;
      const tileY = map.y / 32;
      if (drag.kind === "resize" && drag.corner && imagePlacement) {
        return resizeImagePlacement(
          drag.startPlacement,
          drag.corner,
          tileX,
          tileY,
          imagePlacement.sourceDimensions,
          width,
          height,
        );
      }
      return moveImagePlacement(
        drag.startPlacement,
        tileX - drag.startMap.x,
        tileY - drag.startMap.y,
        width,
        height,
      );
    },
    [height, imagePlacement, width],
  );

  const queueImagePlacement = useCallback(
    (placement: MapImagePlacement) => {
      pendingImagePlacementRef.current = placement;
      if (imageFrameRef.current !== null) return;
      imageFrameRef.current = requestAnimationFrame(() => {
        imageFrameRef.current = null;
        const next = pendingImagePlacementRef.current;
        pendingImagePlacementRef.current = null;
        if (next) onImagePlacement?.(next, false);
      });
    },
    [onImagePlacement],
  );


  const handlePointerDown = useCallback(
    (event: React.PointerEvent<HTMLCanvasElement>) => {
      const screen = screenPoint(event);
      const tile = screenToTile(screen, transform, width, height);
      if (stampPlacement && onStampPlacement && event.button === 0 && tile) {
        const stampWidth =
          stampPlacement.sourceBounds.right - stampPlacement.sourceBounds.left;
        const stampHeight =
          stampPlacement.sourceBounds.bottom - stampPlacement.sourceBounds.top;
        onStampPlacement(
          {
            x: Math.min(tile.x, Math.max(0, width - stampWidth)),
            y: Math.min(tile.y, Math.max(0, height - stampHeight)),
          },
          true,
        );
        event.currentTarget.focus();
        return;
      }
      if (imagePlacement && onImagePlacement && event.button === 0) {
        const imageScreen = mapToScreen(
          {
            x: imagePlacement.placement.x * 32,
            y: imagePlacement.placement.y * 32,
          },
          transform,
        );
        const imageWidth = imagePlacement.placement.width * 32 * transform.zoom;
        const imageHeight = imagePlacement.placement.height * 32 * transform.zoom;
        const corners: Array<[ImageResizeCorner, number, number]> = [
          ["nw", imageScreen.x, imageScreen.y],
          ["ne", imageScreen.x + imageWidth, imageScreen.y],
          ["sw", imageScreen.x, imageScreen.y + imageHeight],
          ["se", imageScreen.x + imageWidth, imageScreen.y + imageHeight],
        ];
        const corner = corners.find(
          ([, x, y]) => Math.abs(screen.x - x) <= 10 && Math.abs(screen.y - y) <= 10,
        );
        const inside =
          screen.x >= imageScreen.x &&
          screen.x <= imageScreen.x + imageWidth &&
          screen.y >= imageScreen.y &&
          screen.y <= imageScreen.y + imageHeight;
        if (corner || inside) {
          const map = screenToMap(screen, transform);
          imageDragRef.current = {
            pointerId: event.pointerId,
            kind: corner ? "resize" : "move",
            corner: corner?.[0],
            startMap: { x: map.x / 32, y: map.y / 32 },
            startPlacement: imagePlacement.placement,
          };
          event.currentTarget.focus();
          event.currentTarget.setPointerCapture(event.pointerId);
          return;
        }
      }
      const panning =
        interactionMode === "pan" || event.button === 1 || event.button === 2;
      if (!tile && !panning) return;
      setPreviewCells(null);
      dragRef.current = {
        pointerId: event.pointerId,
        startScreen: screen,
        startTile: tile ?? { x: 0, y: 0 },
        samples: tile ? [tile] : [],
        transform,
        panning,
        moved: false,
        baseCells: new Set(activeCells),
        shape: event.ctrlKey ? "free" : selectionShape,
        operation:
          event.shiftKey && selectionOperation === "replace"
            ? "add"
            : selectionOperation,
      };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    [
      activeCells,
      height,
      imagePlacement,
      interactionMode,
      onImagePlacement,
      screenPoint,
      onStampPlacement,
      selectionOperation,
      selectionShape,
      transform,
      stampPlacement,
      width,
    ],
  );

  const handlePointerMove = useCallback(
    (event: React.PointerEvent<HTMLCanvasElement>) => {
      const screen = screenPoint(event);
      const tile = screenToTile(screen, transform, width, height);
      queueCursor(tile);
      const imageDrag = imageDragRef.current;
      if (imageDrag?.pointerId === event.pointerId) {
        queueImagePlacement(imagePlacementForPointer(screen, imageDrag));
        return;
      }
      const drag = dragRef.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      const dx = screen.x - drag.startScreen.x;
      const dy = screen.y - drag.startScreen.y;
      if (Math.hypot(dx, dy) >= 4) drag.moved = true;
      if (drag.panning) {
        pendingPanRef.current = {
          ...drag.transform,
          panX: drag.transform.panX + dx,
          panY: drag.transform.panY + dy,
        };
        if (panFrameRef.current === null) {
          panFrameRef.current = requestAnimationFrame(() => {
            panFrameRef.current = null;
            const next = pendingPanRef.current;
            pendingPanRef.current = null;
            if (next) {
              transformRef.current = next;
              setTransform(next);
            }
          });
        }
        return;
      }
      if (!tile) return;
      if (interactionMode === "select") {
        const previous = drag.samples.at(-1);
        if (!previous || previous.x !== tile.x || previous.y !== tile.y) {
          drag.samples.push(tile);
        }
        if (drag.moved) queueSelectionPreview(tile);
      }
    },
    [
      height,
      imagePlacementForPointer,
      interactionMode,
      queueCursor,
      queueImagePlacement,
      queueSelectionPreview,
      screenPoint,
      transform,
      width,
    ],
  );

  const handlePointerUp = useCallback(
    (event: React.PointerEvent<HTMLCanvasElement>) => {
      const imageDrag = imageDragRef.current;
      if (imageDrag?.pointerId === event.pointerId) {
        imageDragRef.current = null;
        const next =
          pendingImagePlacementRef.current ??
          imagePlacementForPointer(screenPoint(event), imageDrag);
        pendingImagePlacementRef.current = null;
        if (imageFrameRef.current !== null) {
          cancelAnimationFrame(imageFrameRef.current);
          imageFrameRef.current = null;
        }
        onImagePlacement?.(next, true);
        return;
      }
      const drag = dragRef.current;
      dragRef.current = null;
      pendingPreviewTileRef.current = null;
      if (previewFrameRef.current !== null) {
        cancelAnimationFrame(previewFrameRef.current);
        previewFrameRef.current = null;
      }
      setPreviewCells(null);
      if (!drag || drag.pointerId !== event.pointerId) return;
      const screen = screenPoint(event);
      let tile = screenToTile(screen, transform, width, height);
      if (drag.panning) return;
      if (!tile && interactionMode === "select") {
        const map = screenToMap(screen, transform);
        tile = {
          x: Math.max(0, Math.min(width - 1, Math.floor(map.x / 32))),
          y: Math.max(0, Math.min(height - 1, Math.floor(map.y / 32))),
        };
      }
      if (!tile) {
        if (interactionMode === "inspect") onObjectSelect(null);
        return;
      }
      if (interactionMode === "inspect" && !drag.moved) {
        const key = `${tile.x},${tile.y}`;
        const previousId = lastHitRef.current.tile === key ? lastHitRef.current.id : undefined;
        const hit = spatialIndex.cycle(tile.x, tile.y, previousId);
        lastHitRef.current = { tile: key, id: hit?.id };
        onObjectSelect(hit);
        return;
      }
      if (interactionMode !== "select") return;
      onActiveCells(
        selectionCellsForGesture({
          baseCells: drag.baseCells,
          start: drag.startTile,
          end: tile,
          samples: drag.samples,
          moved: drag.moved,
          shape: drag.shape,
          operation: drag.operation,
          width,
          height,
        }),
      );
    },
    [
      height,
      imagePlacementForPointer,
      interactionMode,
      onActiveCells,
      onObjectSelect,
      onImagePlacement,
      screenPoint,
      spatialIndex,
      transform,
      width,
    ],
  );

  const handleWheel = useCallback(
    (event: React.WheelEvent<HTMLCanvasElement>) => {
      event.preventDefault();
      const deltaPixels =
        event.deltaY *
        (event.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? 16
          : event.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? size.height
            : 1);
      if (!Number.isFinite(deltaPixels) || deltaPixels === 0) return;
      const rect = event.currentTarget.getBoundingClientRect();
      const factor = Math.exp(
        -Math.max(-600, Math.min(600, deltaPixels)) * 0.0018,
      );
      pendingWheelRef.current = zoomAtPoint(
        pendingWheelRef.current ?? transformRef.current,
        { x: event.clientX - rect.left, y: event.clientY - rect.top },
        factor,
      );
      if (wheelFrameRef.current !== null) return;
      wheelFrameRef.current = requestAnimationFrame(() => {
        wheelFrameRef.current = null;
        const next = pendingWheelRef.current;
        pendingWheelRef.current = null;
        if (!next) return;
        transformRef.current = next;
        setTransform(next);
        onZoom(next.zoom);
      });
    },
    [onZoom, size.height],
  );

  return (
    <div ref={hostRef} className="relative h-full min-h-0 min-w-0 overflow-hidden bg-[#071019]">
      <canvas
        ref={canvasRef}
        tabIndex={0}
        aria-label={ariaLabel}
        className="size-full touch-none outline-none focus-visible:ring-2 focus-visible:ring-emerald-400"
        onContextMenu={(event) => event.preventDefault()}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerLeave={() => queueCursor(null)}
        onPointerCancel={() => {
          const cancelledImageDrag = imageDragRef.current;
          imageDragRef.current = null;
          pendingImagePlacementRef.current = null;
          if (imageFrameRef.current !== null) {
            cancelAnimationFrame(imageFrameRef.current);
            imageFrameRef.current = null;
          }
          if (cancelledImageDrag && imagePlacement) {
            onImagePlacement?.(imagePlacement.placement, true);
          }
          dragRef.current = null;
          pendingPreviewTileRef.current = null;
          if (previewFrameRef.current !== null) {
            cancelAnimationFrame(previewFrameRef.current);
            previewFrameRef.current = null;
          }
          setPreviewCells(null);
        }}
        onWheel={handleWheel}
        onKeyDown={(event) => {
          if (stampPlacement) {
            if (event.key === "Escape") {
              event.preventDefault();
              onStampCancel?.();
              return;
            }
            if (event.key === "Enter" && stampPlacement.canConfirm) {
              event.preventDefault();
              onStampConfirm?.();
              return;
            }
            if (
              event.key === "ArrowLeft" ||
              event.key === "ArrowRight" ||
              event.key === "ArrowUp" ||
              event.key === "ArrowDown"
            ) {
              event.preventDefault();
              const step = event.shiftKey ? 8 : 1;
              const stampWidth =
                stampPlacement.sourceBounds.right - stampPlacement.sourceBounds.left;
              const stampHeight =
                stampPlacement.sourceBounds.bottom - stampPlacement.sourceBounds.top;
              const x =
                stampPlacement.destination.x +
                (event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0);
              const y =
                stampPlacement.destination.y +
                (event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0);
              onStampPlacement?.(
                {
                  x: Math.max(0, Math.min(width - stampWidth, x)),
                  y: Math.max(0, Math.min(height - stampHeight, y)),
                },
                true,
              );
              return;
            }
          }
          if (imagePlacement) {
            if (event.key === "Escape") {
              event.preventDefault();
              onImageCancel?.();
              return;
            }
            if (event.key === "Enter" && imagePlacement.canConfirm) {
              event.preventDefault();
              onImageConfirm?.();
              return;
            }
            if (
              event.key === "ArrowLeft" ||
              event.key === "ArrowRight" ||
              event.key === "ArrowUp" ||
              event.key === "ArrowDown"
            ) {
              event.preventDefault();
              const step = event.shiftKey ? 8 : 1;
              const deltaX =
                event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0;
              const deltaY =
                event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0;
              onImagePlacement?.(
                moveImagePlacement(
                  imagePlacement.placement,
                  deltaX,
                  deltaY,
                  width,
                  height,
                ),
                true,
              );
              return;
            }
          }
          if (event.key === "Escape") onActiveCells(new Set());
        }}
      />
      <div className="absolute bottom-3 right-3 z-10 flex items-center gap-1 rounded-md border border-border bg-background/90 p-1 shadow-lg">
        <button
          type="button"
          className="min-h-11 min-w-11 rounded text-lg hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="축소"
          onClick={() =>
            setTransform((current) => {
              const next = zoomAtPoint(
                current,
                { x: size.width / 2, y: size.height / 2 },
                1 / 1.2,
              );
              onZoom(next.zoom);
              return next;
            })
          }
        >
          −
        </button>
        <button
          type="button"
          className="min-h-11 min-w-11 rounded text-lg hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="확대"
          onClick={() =>
            setTransform((current) => {
              const next = zoomAtPoint(
                current,
                { x: size.width / 2, y: size.height / 2 },
                1.2,
              );
              onZoom(next.zoom);
              return next;
            })
          }
        >
          +
        </button>
        <button
          type="button"
          className="min-h-11 rounded px-3 text-xs hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          aria-label="그리드 표시"
          aria-pressed={showGrid}
          onClick={() => setShowGrid((visible) => !visible)}
        >
          Grid
        </button>
      </div>
      {loading && (
        <div className="pointer-events-none absolute right-3 top-3 rounded-md border border-border bg-background/85 px-2 py-1 text-xs text-muted-foreground">
          뷰포트 렌더링…
        </div>
      )}
      {renderError && (
        <div role="alert" className="absolute left-3 top-3 max-w-sm rounded-md border border-destructive/50 bg-destructive/15 px-3 py-2 text-xs text-destructive-foreground">
          {renderError}
        </div>
      )}
    </div>
  );
}
