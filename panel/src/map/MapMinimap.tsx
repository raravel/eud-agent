import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { LoaderCircle, Map as MapIcon } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { TileViewport } from "./canvasTransform";
import {
  type MapDiffMarker,
  type MapLayer,
  type MapObjectItem,
  type MapRenderSource,
  type MapView,
  type RowSpan,
  type SelectionMask,
} from "./mapProtocol";

export interface MinimapGeometry {
  left: number;
  top: number;
  width: number;
  height: number;
  scale: number;
}

export function fitMinimapGeometry(
  viewportWidth: number,
  viewportHeight: number,
  mapWidth: number,
  mapHeight: number,
): MinimapGeometry {
  const scale = Math.min(
    viewportWidth / Math.max(1, mapWidth),
    viewportHeight / Math.max(1, mapHeight),
  );
  const width = mapWidth * scale;
  const height = mapHeight * scale;
  return {
    left: (viewportWidth - width) / 2,
    top: (viewportHeight - height) / 2,
    width,
    height,
    scale,
  };
}

export function minimapScreenToTile(
  point: { x: number; y: number },
  geometry: MinimapGeometry,
  mapWidth: number,
  mapHeight: number,
): { x: number; y: number } {
  return {
    x: Math.max(0, Math.min(mapWidth, (point.x - geometry.left) / geometry.scale)),
    y: Math.max(0, Math.min(mapHeight, (point.y - geometry.top) / geometry.scale)),
  };
}

const selectionStyle: Record<
  SelectionMask["role"],
  { fill: string; stroke: string }
> = {
  target: { fill: "rgba(16,185,129,.16)", stroke: "#34d399" },
  reference: { fill: "rgba(59,130,246,.12)", stroke: "#60a5fa" },
  protect: { fill: "rgba(244,63,94,.16)", stroke: "#fb7185" },
  anchor: { fill: "rgba(245,158,11,.14)", stroke: "#fbbf24" },
};

export interface MapMinimapProps {
  renderSource: MapRenderSource;
  width: number;
  height: number;
  view: MapView;
  layers: MapLayer[];
  selections: SelectionMask[];
  activeRows: RowSpan[];
  objects: MapObjectItem[];
  diffRows: RowSpan[];
  diffMarkers: MapDiffMarker[];
  viewport: TileViewport | null;
  onNavigate(x: number, y: number): void;
}

export function MapMinimap({
  renderSource,
  width,
  height,
  view,
  layers,
  selections,
  activeRows,
  objects,
  diffRows,
  diffMarkers,
  viewport,
  onNavigate,
}: MapMinimapProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bitmapRef = useRef<ImageBitmap | null>(null);
  const requestSequenceRef = useRef(0);
  const navigationFrameRef = useRef<number | null>(null);
  const pendingNavigationRef = useRef<{ x: number; y: number } | null>(null);
  const draggingRef = useRef<number | null>(null);
  const [size, setSize] = useState({ width: 1, height: 1, ratio: 1 });
  const [bitmap, setBitmap] = useState<ImageBitmap | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const layerKey = [...layers].sort().join(",");
  const geometry = useMemo(
    () => fitMinimapGeometry(size.width, size.height, width, height),
    [height, size.height, size.width, width],
  );

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const observer = new ResizeObserver(([entry]) => {
      setSize({
        width: Math.max(1, Math.floor(entry.contentRect.width)),
        height: Math.max(1, Math.floor(entry.contentRect.height)),
        ratio: window.devicePixelRatio || 1,
      });
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const sequence = ++requestSequenceRef.current;
    let disposed = false;
    setLoading(true);
    setError("");
    const render = () => {
      void renderSource
        .render({
          x: 0,
          y: 0,
          width,
          height,
          scale: 8,
          layers,
        })
        .then((blob) => createImageBitmap(blob))
        .then((nextBitmap) => {
          if (disposed || requestSequenceRef.current !== sequence) {
            nextBitmap.close();
            return;
          }
          bitmapRef.current?.close();
          bitmapRef.current = nextBitmap;
          setBitmap(nextBitmap);
        })
        .catch((reason) => {
          if (!disposed && requestSequenceRef.current === sequence) {
            setError(String(reason));
          }
        })
        .finally(() => {
          if (!disposed && requestSequenceRef.current === sequence) {
            setLoading(false);
          }
        });
    };
    const timer = view === "draft" ? window.setTimeout(render, 120) : null;
    if (timer === null) render();
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [height, layerKey, layers, renderSource, view, width]);

  useEffect(
    () => () => {
      requestSequenceRef.current += 1;
      bitmapRef.current?.close();
      if (navigationFrameRef.current !== null) {
        cancelAnimationFrame(navigationFrameRef.current);
      }
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
    context.fillStyle = "#050a11";
    context.fillRect(0, 0, size.width, size.height);
    context.imageSmoothingEnabled = true;
    if (bitmap) {
      context.drawImage(
        bitmap,
        geometry.left,
        geometry.top,
        geometry.width,
        geometry.height,
      );
    }

    if (layers.includes("locations")) {
      context.strokeStyle = "rgba(34,211,238,.7)";
      context.lineWidth = 1;
      for (const item of objects) {
        if (!item.location) continue;
        const [left, top, right, bottom] = item.location.tileRect;
        context.strokeRect(
          geometry.left + left * geometry.scale,
          geometry.top + top * geometry.scale,
          (right - left) * geometry.scale,
          (bottom - top) * geometry.scale,
        );
      }
    }

    if (view === "diff") {
      if (layers.includes("terrain")) {
        context.fillStyle = "rgba(236,72,153,.42)";
        for (const row of diffRows) {
          for (const [left, right] of row.spans) {
            context.fillRect(
              geometry.left + left * geometry.scale,
              geometry.top + row.y * geometry.scale,
              (right - left) * geometry.scale,
              Math.max(1, geometry.scale),
            );
          }
        }
      }
      context.strokeStyle = "#f472b6";
      context.lineWidth = 1.5;
      for (const marker of diffMarkers.filter((item) => layers.includes(item.layer))) {
        context.strokeRect(
          geometry.left + marker.bounds.left * geometry.scale,
          geometry.top + marker.bounds.top * geometry.scale,
          (marker.bounds.right - marker.bounds.left) * geometry.scale,
          (marker.bounds.bottom - marker.bounds.top) * geometry.scale,
        );
      }
    }

    for (const selection of selections) {
      const style = selectionStyle[selection.role];
      context.fillStyle = style.fill;
      context.strokeStyle = style.stroke;
      context.lineWidth = selection.role === "protect" ? 1.5 : 1;
      for (const row of selection.rows) {
        for (const [left, right] of row.spans) {
          const x = geometry.left + left * geometry.scale;
          const y = geometry.top + row.y * geometry.scale;
          const spanWidth = (right - left) * geometry.scale;
          context.fillRect(x, y, spanWidth, Math.max(1, geometry.scale));
          if (geometry.scale >= 1) {
            context.strokeRect(x, y, spanWidth, Math.max(1, geometry.scale));
          }
        }
      }
    }

    context.fillStyle = "rgba(250,204,21,.34)";
    for (const row of activeRows) {
      for (const [left, right] of row.spans) {
        context.fillRect(
          geometry.left + left * geometry.scale,
          geometry.top + row.y * geometry.scale,
          (right - left) * geometry.scale,
          Math.max(1, geometry.scale),
        );
      }
    }

    if (viewport) {
      context.fillStyle = "rgba(255,255,255,.05)";
      context.strokeStyle = "#f8fafc";
      context.lineWidth = 2;
      const left = geometry.left + viewport.left * geometry.scale;
      const top = geometry.top + viewport.top * geometry.scale;
      const viewportWidth = Math.max(2, (viewport.right - viewport.left) * geometry.scale);
      const viewportHeight = Math.max(2, (viewport.bottom - viewport.top) * geometry.scale);
      context.fillRect(left, top, viewportWidth, viewportHeight);
      context.strokeRect(left, top, viewportWidth, viewportHeight);
    }
  }, [
    activeRows,
    bitmap,
    diffMarkers,
    diffRows,
    geometry,
    layers,
    objects,
    selections,
    size,
    view,
    viewport,
  ]);

  useEffect(() => {
    const frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [draw]);

  const queueNavigation = useCallback(
    (point: { x: number; y: number }) => {
      pendingNavigationRef.current = point;
      if (navigationFrameRef.current !== null) return;
      navigationFrameRef.current = requestAnimationFrame(() => {
        navigationFrameRef.current = null;
        const next = pendingNavigationRef.current;
        pendingNavigationRef.current = null;
        if (next) onNavigate(next.x, next.y);
      });
    },
    [onNavigate],
  );

  const navigateFromPointer = useCallback(
    (event: React.PointerEvent<HTMLCanvasElement>) => {
      const rect = event.currentTarget.getBoundingClientRect();
      queueNavigation(
        minimapScreenToTile(
          { x: event.clientX - rect.left, y: event.clientY - rect.top },
          geometry,
          width,
          height,
        ),
      );
    },
    [geometry, height, queueNavigation, width],
  );

  const viewportCenter = viewport
    ? {
        x: (viewport.left + viewport.right) / 2,
        y: (viewport.top + viewport.bottom) / 2,
      }
    : { x: width / 2, y: height / 2 };

  return (
    <section className="flex h-full min-h-0 min-w-0 flex-col border-t border-border bg-card/70">
      <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border px-2">
        <MapIcon className="size-3.5 text-cyan-300" aria-hidden="true" />
        <strong className="text-xs">미니맵</strong>
        <Badge variant="outline" className="ml-auto h-5 px-1.5 text-[10px]">
          {view === "original"
            ? "원본"
            : view === "candidate"
              ? "후보"
              : view === "draft"
                ? "수정 중"
                : "차이"}
        </Badge>
      </header>
      <div ref={hostRef} className="relative min-h-0 min-w-0 flex-1 overflow-hidden">
        <canvas
          ref={canvasRef}
          tabIndex={0}
          aria-label="미니맵 — 클릭하거나 드래그하여 메인 캔버스 이동"
          title={
            viewport
              ? `현재 화면 ${Math.floor(viewport.left)},${Math.floor(viewport.top)}–${Math.ceil(viewport.right)},${Math.ceil(viewport.bottom)}`
              : "현재 화면 계산 중"
          }
          className="size-full cursor-crosshair touch-none outline-none focus-visible:ring-2 focus-visible:ring-cyan-400"
          onPointerDown={(event) => {
            draggingRef.current = event.pointerId;
            event.currentTarget.setPointerCapture(event.pointerId);
            navigateFromPointer(event);
          }}
          onPointerMove={(event) => {
            if (draggingRef.current === event.pointerId) navigateFromPointer(event);
          }}
          onPointerUp={(event) => {
            if (draggingRef.current === event.pointerId) {
              navigateFromPointer(event);
              draggingRef.current = null;
            }
          }}
          onPointerCancel={() => {
            draggingRef.current = null;
          }}
          onKeyDown={(event) => {
            const step = event.shiftKey ? 8 : 1;
            const next =
              event.key === "Home"
                ? { x: width / 2, y: height / 2 }
                : event.key === "ArrowLeft"
                  ? { x: viewportCenter.x - step, y: viewportCenter.y }
                  : event.key === "ArrowRight"
                    ? { x: viewportCenter.x + step, y: viewportCenter.y }
                    : event.key === "ArrowUp"
                      ? { x: viewportCenter.x, y: viewportCenter.y - step }
                      : event.key === "ArrowDown"
                        ? { x: viewportCenter.x, y: viewportCenter.y + step }
                        : null;
            if (!next) return;
            event.preventDefault();
            onNavigate(
              Math.max(0, Math.min(width, next.x)),
              Math.max(0, Math.min(height, next.y)),
            );
          }}
        />
        {loading && (
          <div className="pointer-events-none absolute right-1.5 top-1.5 flex items-center gap-1 rounded bg-background/85 px-1.5 py-1 text-[10px] text-muted-foreground">
            <LoaderCircle className="size-3 animate-spin" aria-hidden="true" />
            갱신 중
          </div>
        )}
        {error && (
          <div role="alert" className="absolute inset-x-2 bottom-2 rounded border border-destructive/40 bg-background/90 px-2 py-1 text-[10px] text-destructive">
            미니맵을 불러오지 못했습니다: {error}
          </div>
        )}
      </div>
    </section>
  );
}
