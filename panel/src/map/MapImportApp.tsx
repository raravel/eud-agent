import { useCallback, useEffect, useMemo, useState } from "react";
import { Layers3, LoaderCircle, MapPinned, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { MapCanvas } from "./MapCanvas";
import { MapImportSelectionControls } from "./MapImportSelectionControls";
import { MapImportToolbar } from "./MapImportToolbar";
import { MapMinimap } from "./MapMinimap";
import {
  mapImportBootstrap,
  mapImportRenderSource,
  mapImportSourceObjects,
  mapImportSourcePick,
  mapImportStampList,
  mapImportStampSave,
  type ImportedStampView,
  type MapImportBootstrap,
  type MapImportSource,
} from "./importProtocol";
import {
  cellsToRows,
  selectionBounds,
} from "./selectionMask";
import type { TileViewport } from "./canvasTransform";
import type {
  MapLayer,
  MapObjectItem,
  SelectionOperation,
  SelectionShape,
} from "./mapProtocol";

const allLayers: MapLayer[] = [
  "terrain",
  "units",
  "buildings",
  "doodads",
  "sprites",
  "locations",
];
const objectLayers: Array<Exclude<MapLayer, "terrain">> = [
  "units",
  "buildings",
  "doodads",
  "sprites",
  "locations",
];

async function loadAllSourceObjects(sourceId: string): Promise<MapObjectItem[]> {
  const objects: MapObjectItem[] = [];
  for (const layer of objectLayers) {
    let offset = 0;
    for (;;) {
      const page = await mapImportSourceObjects({
        sourceId,
        layer,
        offset,
        limit: 500,
      });
      objects.push(...page.items);
      offset += page.items.length;
      if (page.items.length === 0 || offset >= page.total) break;
    }
  }
  return objects;
}

export default function MapImportApp() {
  const [bootstrap, setBootstrap] = useState<MapImportBootstrap | null>(null);
  const [source, setSource] = useState<MapImportSource | null>(null);
  const [objects, setObjects] = useState<MapObjectItem[]>([]);
  const [entries, setEntries] = useState<ImportedStampView[]>([]);
  const [activeCells, setActiveCells] = useState<Set<string>>(new Set());
  const [shape, setShape] = useState<SelectionShape>("rectangle");
  const [operation, setOperation] = useState<SelectionOperation>("replace");
  const [copyLayers, setCopyLayers] = useState<MapLayer[]>(allLayers);
  const [visibleLayers, setVisibleLayers] = useState<MapLayer[]>(allLayers);
  const [label, setLabel] = useState("가져온 영역 A");
  const [viewport, setViewport] = useState<TileViewport | null>(null);
  const [viewportTarget, setViewportTarget] = useState<{
    x: number;
    y: number;
    sequence: number;
  }>();
  const [picking, setPicking] = useState(false);
  const [saving, setSaving] = useState(false);
  const [stale, setStale] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const refreshEntries = useCallback(async () => {
    const next = await mapImportStampList();
    setEntries(next);
  }, []);

  useEffect(() => {
    let disposed = false;
    void Promise.all([mapImportBootstrap(), mapImportStampList()])
      .then(([nextBootstrap, nextEntries]) => {
        if (disposed) return;
        setBootstrap(nextBootstrap);
        setEntries(nextEntries);
      })
      .catch((reason) => {
        if (!disposed) setError(String(reason));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!bootstrap || !source) return;
    let disposed = false;
    const check = () => {
      void mapImportBootstrap()
        .then((current) => {
          if (disposed) return;
          setStale(
            current.destination.projectId !== bootstrap.destination.projectId ||
              current.destination.fileSha256 !== bootstrap.destination.fileSha256,
          );
        })
        .catch(() => {
          if (!disposed) setStale(true);
        });
    };
    const timer = window.setInterval(check, 2_000);
    check();
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [bootstrap, source]);

  const renderSource = useMemo(
    () => (source ? mapImportRenderSource(source) : null),
    [source],
  );
  const bounds = useMemo(() => selectionBounds(activeCells), [activeCells]);
  const compatible =
    bootstrap !== null && source !== null && source.tileset === bootstrap.destination.tileset;
  const objectCounts = useMemo(() => {
    const counts = { units: 0, buildings: 0, doodads: 0, sprites: 0, locations: 0 };
    for (const item of objects) {
      if (item.location) counts.locations += 1;
      else if (item.objectRef?.kind === "unit") counts.units += 1;
      else if (item.objectRef?.kind === "building") counts.buildings += 1;
      else if (item.objectRef?.kind === "doodad") counts.doodads += 1;
      else if (item.objectRef?.kind === "sprite") counts.sprites += 1;
    }
    return counts;
  }, [objects]);

  const pickSource = useCallback(async () => {
    setPicking(true);
    setError("");
    try {
      const picked = await mapImportSourcePick();
      if (!picked) return;
      setSource(picked);
      setActiveCells(new Set());
      setViewport(null);
      setStale(false);
      setObjects(await loadAllSourceObjects(picked.sourceId));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setPicking(false);
    }
  }, []);

  const save = useCallback(async () => {
    if (!source || !compatible || stale || activeCells.size === 0) return;
    setSaving(true);
    setError("");
    try {
      await mapImportStampSave({
        sourceId: source.sourceId,
        label,
        rows: cellsToRows(activeCells),
        layers: copyLayers,
      });
      await refreshEntries();
      setActiveCells(new Set());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  }, [activeCells, compatible, copyLayers, label, refreshEntries, source, stale]);

  if (loading || !bootstrap) {
    return (
      <main className="grid h-dvh place-items-center overflow-hidden bg-background text-sm text-muted-foreground">
        <span className="flex items-center gap-2">
          <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
          저장된 대상 맵 확인 중…
        </span>
      </main>
    );
  }

  return (
    <main className="flex h-dvh min-w-0 flex-col overflow-hidden bg-background text-foreground">
      <MapImportToolbar
        destination={bootstrap.destination}
        source={source}
        picking={picking}
        stale={stale}
        onPick={() => void pickSource()}
      />
      {error && (
        <div role="alert" className="flex min-w-0 items-center gap-2 border-b border-destructive/50 bg-destructive/10 px-4 py-2 text-sm text-destructive">
          <span className="min-w-0 flex-1 break-words">{error}</span>
          <Button type="button" size="icon" variant="ghost" aria-label="오류 닫기" onClick={() => setError("")}>
            <X className="size-4" aria-hidden="true" />
          </Button>
        </div>
      )}
      {!source || !renderSource ? (
        <section className="grid min-h-0 flex-1 place-items-center p-6 text-center">
          <div>
            <MapPinned className="mx-auto size-12 text-muted-foreground" aria-hidden="true" />
            <h1 className="mt-3 text-lg font-semibold">읽기 전용 Map Importer</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              SCX/SCM을 선택하면 원본 대신 content-addressed pinned blob만 렌더·저장·배치에 사용합니다.
            </p>
            <Button type="button" className="mt-4" onClick={() => void pickSource()}>
              SCX/SCM 선택
            </Button>
          </div>
        </section>
      ) : (
        <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[260px_minmax(0,1fr)] max-lg:grid-cols-[230px_minmax(0,1fr)]">
          <MapImportSelectionControls
            shape={shape}
            operation={operation}
            layers={copyLayers}
            label={label}
            selectedCells={activeCells.size}
            bounds={bounds}
            saving={saving}
            disabled={!compatible || stale}
            onShape={setShape}
            onOperation={setOperation}
            onLayers={setCopyLayers}
            onLabel={setLabel}
            onClear={() => setActiveCells(new Set())}
            onSave={() => void save()}
          />
          <section className="grid min-h-0 min-w-0 grid-rows-[minmax(0,1fr)_220px]">
            <div className="min-h-0 min-w-0 p-3">
              <MapCanvas
                renderSource={renderSource}
                ariaLabel="가져오기 소스 맵 캔버스"
                width={source.width}
                height={source.height}
                view="candidate"
                layers={visibleLayers}
                selections={[]}
                activeCells={activeCells}
                selectionShape={shape}
                selectionOperation={operation}
                interactionMode="select"
                objects={objects}
                diffRows={[]}
                diffMarkers={[]}
                viewportTarget={viewportTarget}
                onActiveCells={setActiveCells}
                onCursor={() => {}}
                onObjectSelect={() => {}}
                onZoom={() => {}}
                onSelectionAnchor={() => {}}
                onViewportChange={setViewport}
              />
            </div>
            <div className="grid min-h-0 min-w-0 grid-cols-[minmax(280px,420px)_minmax(0,1fr)] gap-3 border-t border-border p-3 max-xl:grid-cols-[320px_minmax(0,1fr)]">
              <MapMinimap
                renderSource={renderSource}
                width={source.width}
                height={source.height}
                view="candidate"
                layers={visibleLayers}
                selections={[]}
                activeRows={cellsToRows(activeCells)}
                objects={objects}
                diffRows={[]}
                diffMarkers={[]}
                viewport={viewport}
                onNavigate={(x, y) =>
                  setViewportTarget((current) => ({
                    x,
                    y,
                    sequence: (current?.sequence ?? 0) + 1,
                  }))
                }
              />
              <div className="min-w-0 overflow-y-auto rounded-lg border border-border bg-card/60 p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <Layers3 className="size-4 text-muted-foreground" aria-hidden="true" />
                  <span className="text-sm font-semibold">표시 레이어</span>
                  {allLayers.map((layer) => (
                    <Button
                      key={layer}
                      type="button"
                      size="sm"
                      variant={visibleLayers.includes(layer) ? "secondary" : "outline"}
                      aria-pressed={visibleLayers.includes(layer)}
                      onClick={() =>
                        setVisibleLayers((current) =>
                          current.includes(layer)
                            ? current.filter((value) => value !== layer)
                            : [...current, layer],
                        )
                      }
                    >
                      {layer}
                    </Button>
                  ))}
                </div>
                <div className="mt-3 flex flex-wrap gap-2 text-xs">
                  {Object.entries(objectCounts).map(([layer, count]) => (
                    <Badge key={layer} variant="outline">{layer} {count}</Badge>
                  ))}
                </div>
                <div className="mt-3 border-t border-border pt-3">
                  <p className="text-sm font-semibold">프로젝트 가져온 영역 · {entries.length}</p>
                  <div className="mt-2 flex flex-wrap gap-2">
                    {entries.map((entry) => (
                      <Badge key={entry.id} variant={entry.available && entry.compatible ? "secondary" : "destructive"}>
                        {entry.label} · {entry.bounds.right - entry.bounds.left}×{entry.bounds.bottom - entry.bounds.top}
                      </Badge>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          </section>
        </div>
      )}
    </main>
  );
}
