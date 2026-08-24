import { useEffect, useMemo, useRef, useState } from "react";
import {
  Building2,
  CopyPlus,
  ImageOff,
  Layers3,
  MapPinPlus,
  Mountain,
  Plus,
  Search,
  Sparkles,
  Trash2,
  Users,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import {
  mapImportStampThumbnail,
  type ImportedStampView,
} from "./importProtocol";
import {
  mapCatalog,
  mapThumbnail,
  type MapLayer,
  type MapLocation,
  type PaletteEntry,
  type PaletteKind,
  type SavedSelection,
  type Tileset,
} from "./mapProtocol";

interface PaletteTab {
  layer: MapLayer;
  label: string;
  icon: typeof Mountain;
}

const tabs: PaletteTab[] = [
  { layer: "terrain", label: "지형", icon: Mountain },
  { layer: "buildings", label: "건물", icon: Building2 },
  { layer: "units", label: "유닛", icon: Users },
  { layer: "doodads", label: "두다드", icon: Layers3 },
  { layer: "sprites", label: "스프라이트", icon: Sparkles },
  { layer: "locations", label: "로케이션", icon: MapPinPlus },
];

export interface MapPaletteProps {
  sessionId: string;
  tileset: Tileset;
  locations: MapLocation[];
  selections: SavedSelection[];
  importedEntries: ImportedStampView[];
  onMention(entry: PaletteEntry, layer: MapLayer, kind: PaletteKind): void;
  onStampMention(selection: SavedSelection): void;
  onStampPlace(selection: SavedSelection): void;
  onImportedMention(stamp: ImportedStampView): void;
  onImportedPlace(stamp: ImportedStampView): void;
  onImportedDelete(stamp: ImportedStampView): void;
  onLocation(location: MapLocation): void;
  onNewLocation(): void;
}

interface ThumbnailCommand {
  sessionId: string;
  layer: MapLayer;
  id: number;
}

interface ThumbnailJob {
  key: string;
  command: ThumbnailCommand;
  promise: Promise<Blob>;
  resolve(blob: Blob): void;
  reject(reason?: unknown): void;
  consumers: number;
  started: boolean;
}

const thumbnailCache = new Map<string, Blob>();
const thumbnailJobs = new Map<string, ThumbnailJob>();
const thumbnailQueue: ThumbnailJob[] = [];
let activeThumbnailJobs = 0;

function pumpThumbnailQueue(): void {
  if (activeThumbnailJobs > 0) return;
  let job = thumbnailQueue.shift();
  while (job && job.consumers === 0) {
    thumbnailJobs.delete(job.key);
    job = thumbnailQueue.shift();
  }
  if (!job) return;
  job.started = true;
  activeThumbnailJobs += 1;
  void mapThumbnail(job.command)
    .then((blob) => {
      thumbnailCache.delete(job.key);
      thumbnailCache.set(job.key, blob);
      while (thumbnailCache.size > 256) {
        const oldest = thumbnailCache.keys().next().value;
        if (oldest === undefined) break;
        thumbnailCache.delete(oldest);
      }
      job.resolve(blob);
    })
    .catch(job.reject)
    .finally(() => {
      activeThumbnailJobs -= 1;
      thumbnailJobs.delete(job.key);
      pumpThumbnailQueue();
    });
}

function acquireThumbnail(command: ThumbnailCommand): {
  promise: Promise<Blob>;
  release(): void;
} {
  const key = `${command.sessionId}|${command.layer}|${command.id}`;
  const cached = thumbnailCache.get(key);
  if (cached) {
    thumbnailCache.delete(key);
    thumbnailCache.set(key, cached);
    return { promise: Promise.resolve(cached), release() {} };
  }
  let job = thumbnailJobs.get(key);
  if (!job) {
    let resolve!: (blob: Blob) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<Blob>((onResolve, onReject) => {
      resolve = onResolve;
      reject = onReject;
    });
    job = {
      key,
      command,
      promise,
      resolve,
      reject,
      consumers: 0,
      started: false,
    };
    thumbnailJobs.set(key, job);
    thumbnailQueue.push(job);
  }
  job.consumers += 1;
  pumpThumbnailQueue();
  let released = false;
  return {
    promise: job.promise,
    release() {
      if (released) return;
      released = true;
      job.consumers -= 1;
      if (!job.started && job.consumers === 0) pumpThumbnailQueue();
    },
  };
}

function PaletteThumbnail({
  sessionId,
  layer,
  id,
}: {
  sessionId: string;
  layer: MapLayer;
  id: number;
}) {
  const hostRef = useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = useState(false);
  const [url, setUrl] = useState("");
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "120px 0px" },
    );
    observer.observe(host);
    return () => observer.disconnect();
  }, [id, layer, sessionId]);

  useEffect(() => {
    if (!visible) return;
    let active = true;
    let objectUrl = "";
    setUrl("");
    setFailed(false);
    const request = acquireThumbnail({ sessionId, layer, id });
    void request.promise
      .then((blob) => {
        if (!active) return;
        objectUrl = URL.createObjectURL(blob);
        setUrl(objectUrl);
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => {
      active = false;
      request.release();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [id, layer, sessionId, visible]);

  return (
    <span
      ref={hostRef}
      className="flex size-14 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground"
    >
      {failed ? (
        <ImageOff className="size-5" aria-hidden="true" />
      ) : url ? (
        <img
          src={url}
          alt=""
          className="size-full rounded-md bg-[#111827] object-contain [image-rendering:pixelated]"
        />
      ) : (
        <Spinner />
      )}
    </span>
  );
}

function ImportedStampThumbnail({ stamp }: { stamp: ImportedStampView }) {
  const [url, setUrl] = useState("");
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!stamp.available) return;
    let disposed = false;
    let objectUrl = "";
    void mapImportStampThumbnail(stamp.id)
      .then((blob) => {
        if (disposed) return;
        objectUrl = URL.createObjectURL(blob);
        setUrl(objectUrl);
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });
    return () => {
      disposed = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [stamp.available, stamp.id, stamp.snapshotHash]);
  return (
    <span className="grid size-16 shrink-0 place-items-center overflow-hidden rounded border border-border bg-muted/50">
      {url ? (
        <img src={url} alt="" className="size-full object-cover [image-rendering:pixelated]" />
      ) : failed || !stamp.available ? (
        <ImageOff className="size-5 text-muted-foreground" aria-hidden="true" />
      ) : (
        <Spinner />
      )}
    </span>
  );
}

export function MapPalette({
  sessionId,
  tileset,
  locations,
  selections,
  importedEntries,
  onMention,
  onStampMention,
  onStampPlace,
  onImportedMention,
  onImportedPlace,
  onImportedDelete,
  onLocation,
  onNewLocation,
}: MapPaletteProps) {
  const [layer, setLayer] = useState<MapLayer>("terrain");
  const [terrainMode, setTerrainMode] = useState<"tiles" | "brushes">("tiles");
  const [query, setQuery] = useState("");
  const [entries, setEntries] = useState<PaletteEntry[]>([]);
  const [offset, setOffset] = useState(0);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const kind = layer === "terrain" ? terrainMode : layer;

  useEffect(() => {
    if (layer === "locations") return;
    let active = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      void mapCatalog({ sessionId, kind, query, offset, limit: 100 })
        .then((result) => {
          if (!active) return;
          setEntries(result.entries);
          setTotal(result.total);
          setError("");
        })
        .catch((reason) => {
          if (active) setError(String(reason));
        })
        .finally(() => {
          if (active) setLoading(false);
        });
    }, 180);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [kind, layer, offset, query, sessionId]);

  const filteredLocations = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return locations;
    return locations.filter((location) =>
      `${location.id} ${location.name}`.toLocaleLowerCase().includes(normalized),
    );
  }, [locations, query]);
  const filteredSelections = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return selections;
    return selections.filter((selection) =>
      `${selection.label} ${selection.role} ${selection.layers.join(" ")}`
        .toLocaleLowerCase()
        .includes(normalized),
    );
  }, [query, selections]);
  const filteredImported = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return importedEntries;
    return importedEntries.filter((stamp) =>
      `${stamp.label} ${stamp.sourceDisplayName} ${stamp.layers.join(" ")}`
        .toLocaleLowerCase()
        .includes(normalized),
    );
  }, [importedEntries, query]);


  const mentionKind: PaletteKind =
    layer === "terrain"
      ? terrainMode === "tiles"
        ? "exactTile"
        : "semanticTerrain"
      : layer === "units"
        ? "unit"
        : layer === "buildings"
          ? "building"
          : layer === "doodads"
            ? "doodad"
            : "sprite";
  const exactTerrain = layer === "terrain" && terrainMode === "tiles";

  return (
    <aside className="flex h-full min-h-0 min-w-0 flex-col border-r border-border bg-card/60">
      <div className="border-b border-border p-2">
        <div className="grid grid-cols-3 gap-1" role="tablist" aria-label="맵 팔레트 레이어">
          {tabs.map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.layer}
                type="button"
                role="tab"
                aria-selected={layer === tab.layer}
                className={cn(
                  "flex min-h-11 items-center justify-center gap-1 rounded-md px-1 text-[11px] font-medium transition-colors",
                  layer === tab.layer
                    ? "bg-primary/15 text-primary"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
                onClick={() => {
                  setLayer(tab.layer);
                  setOffset(0);
                  setEntries([]);
                  setTotal(0);
                }}
              >
                <Icon className="size-4" aria-hidden="true" />
                {tab.label}
              </button>
            );
          })}
        </div>
      </div>

      <div className="space-y-2 border-b border-border p-3">
        <label className="relative block">
          <span className="sr-only">팔레트 검색</span>
          <Search className="pointer-events-none absolute left-3 top-2.5 size-4 text-muted-foreground" aria-hidden="true" />
          <Input
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setOffset(0);
              setEntries([]);
              setTotal(0);
            }}
            className="pl-9"
            placeholder="타입 또는 ID 검색"
          />
        </label>
        {layer === "terrain" && (
          <div
            className="flex min-h-11 flex-wrap items-center gap-1"
            role="group"
            aria-label="지형 팔레트 표시 방식"
          >
            <Button
              type="button"
              size="sm"
              variant={terrainMode === "tiles" ? "secondary" : "ghost"}
              aria-pressed={terrainMode === "tiles"}
              onClick={() => {
                setTerrainMode("tiles");
                setOffset(0);
                setEntries([]);
                setTotal(0);
              }}
            >
              개별 타일
            </Button>
            <Button
              type="button"
              size="sm"
              variant={terrainMode === "brushes" ? "secondary" : "ghost"}
              aria-pressed={terrainMode === "brushes"}
              onClick={() => {
                setTerrainMode("brushes");
                setOffset(0);
                setEntries([]);
                setTotal(0);
              }}
            >
              지형 브러시
            </Button>
            <span className="ml-auto text-[10px] text-muted-foreground">{tileset}</span>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        <section className="mb-2 space-y-2 rounded-lg border border-primary/25 bg-primary/5 p-2" aria-label="영역 스탬프">
          <div className="flex items-center gap-2 px-1">
            <CopyPlus className="size-4 text-primary" aria-hidden="true" />
            <h2 className="text-xs font-semibold text-foreground">영역 스탬프</h2>
            <span className="ml-auto text-[10px] text-muted-foreground">현재 후보 내용</span>
          </div>
          {filteredSelections.length === 0 ? (
            <p className="rounded-md border border-dashed border-border px-3 py-4 text-center text-[11px] leading-relaxed text-muted-foreground">
              저장된 영역이 생기면 이곳에 자동 등록됩니다.
            </p>
          ) : (
            filteredSelections.map((selection) => {
              const width = selection.bounds.right - selection.bounds.left;
              const height = selection.bounds.bottom - selection.bounds.top;
              const layerLabel =
                selection.layers.length === 0 ? "전체 레이어" : selection.layers.join(" · ");
              return (
                <article key={selection.id} className="rounded-md border border-border bg-background/60 p-2">
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <h3 className="truncate text-xs font-medium" title={selection.label}>
                        {selection.label}
                      </h3>
                      <p className="mt-1 font-mono text-[10px] text-muted-foreground">
                        {width}×{height} · {selection.selectedCells.toLocaleString()}셀
                      </p>
                      <p className="mt-1 truncate text-[10px] text-muted-foreground" title={layerLabel}>
                        {layerLabel}
                      </p>
                    </div>
                    <Button
                      type="button"
                      size="sm"
                      variant="secondary"
                      className="min-h-11"
                      aria-label={`${selection.label} 스탬프 배치`}
                      onClick={() => onStampPlace(selection)}
                    >
                      <CopyPlus className="size-4" aria-hidden="true" />
                      배치
                    </Button>
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      className="min-h-11 min-w-11"
                      aria-label={`${selection.label} 스탬프를 프롬프트에 추가`}
                      onClick={() => onStampMention(selection)}
                    >
                      <Plus className="size-4" aria-hidden="true" />
                    </Button>
                  </div>
                </article>
              );
            })
          )}
        </section>
        <section className="mb-2 space-y-2 rounded-lg border border-cyan-500/25 bg-cyan-500/5 p-2" aria-label="가져온 영역">
          <div className="flex items-center gap-2 px-1">
            <CopyPlus className="size-4 text-cyan-400" aria-hidden="true" />
            <h2 className="text-xs font-semibold text-foreground">가져온 영역</h2>
            <span className="ml-auto text-[10px] text-muted-foreground">외부 맵 고정 스냅샷</span>
          </div>
          {filteredImported.length === 0 ? (
            <p className="rounded-md border border-dashed border-border px-3 py-4 text-center text-[11px] leading-relaxed text-muted-foreground">
              다른 맵에서 가져온 영역이 없습니다.
            </p>
          ) : (
            filteredImported.map((stamp) => {
              const width = stamp.bounds.right - stamp.bounds.left;
              const height = stamp.bounds.bottom - stamp.bounds.top;
              const enabled = stamp.available && stamp.compatible;
              return (
                <article key={stamp.id} className="rounded-md border border-border bg-background/60 p-2">
                  <div className="flex min-w-0 items-start gap-2">
                    <ImportedStampThumbnail stamp={stamp} />
                    <div className="min-w-0 flex-1">
                      <h3 className="truncate text-xs font-medium" title={stamp.label}>{stamp.label}</h3>
                      <p className="mt-1 truncate text-[10px] text-muted-foreground" title={stamp.sourceDisplayName}>
                        {stamp.sourceDisplayName} · {stamp.sourceTileset}
                      </p>
                      <p className="mt-1 font-mono text-[10px] text-muted-foreground">
                        {width}×{height} · {stamp.selectedCells.toLocaleString()}셀
                      </p>
                      <p className="mt-1 truncate text-[10px] text-muted-foreground">
                        {stamp.layers.length === 0 ? "전체 레이어" : stamp.layers.join(" · ")}
                      </p>
                      {!enabled && (
                        <p className="mt-1 text-[10px] text-destructive">
                          {stamp.unavailableReason ?? "현재 대상과 호환되지 않습니다."}
                        </p>
                      )}
                    </div>
                  </div>
                  <div className="mt-2 flex justify-end gap-1">
                    <Button type="button" size="sm" variant="secondary" disabled={!enabled} onClick={() => onImportedPlace(stamp)}>
                      <CopyPlus className="size-4" aria-hidden="true" />
                      배치
                    </Button>
                    <Button type="button" size="sm" variant="outline" disabled={!enabled} onClick={() => onImportedMention(stamp)}>
                      <Plus className="size-4" aria-hidden="true" />
                      멘션 추가
                    </Button>
                    <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label={`${stamp.label} 가져온 영역 삭제`} onClick={() => onImportedDelete(stamp)}>
                      <Trash2 className="size-4" aria-hidden="true" />
                    </Button>
                  </div>
                </article>
              );
            })
          )}
        </section>
        {error && <p role="alert" className="m-2 rounded-md bg-destructive/15 p-2 text-xs text-destructive-foreground">{error}</p>}
        {loading && <div className="flex items-center gap-2 p-3 text-xs text-muted-foreground"><Spinner />팔레트 로딩…</div>}
        {layer === "locations" ? (
          <div className="space-y-2">
            <Button type="button" variant="outline" className="h-12 w-full justify-start" onClick={onNewLocation}>
              <MapPinPlus className="size-4" aria-hidden="true" />
              새 로케이션 멘션
            </Button>
            {filteredLocations.map((location) => (
              <button
                key={location.id}
                type="button"
                className="flex min-h-12 w-full items-center gap-2 rounded-md border border-border bg-background/40 px-3 text-left text-xs hover:bg-muted"
                onClick={() => onLocation(location)}
              >
                <span className="font-mono text-cyan-300">#{location.id}</span>
                <span className="min-w-0 flex-1 truncate">{location.name || "이름 없음"}</span>
                {location.anywhere && <span className="text-[10px] text-amber-300">읽기 전용</span>}
                <Plus className="size-4" aria-hidden="true" />
              </button>
            ))}
          </div>
        ) : (
          <div className={exactTerrain ? "grid grid-cols-[repeat(auto-fill,minmax(4.5rem,1fr))] gap-2" : "space-y-2"}>
            {entries.map((entry) => {
              const thumbnailId =
                layer === "terrain" && terrainMode === "brushes"
                  ? ((entry as PaletteEntry & { previewTile?: number }).previewTile ?? entry.id)
                  : entry.id;
              if (exactTerrain) {
                return (
                  <button
                    key={entry.id}
                    type="button"
                    className="group relative flex min-h-24 min-w-0 flex-col items-center gap-1 rounded-lg border border-border bg-background/40 p-2 text-center transition-colors hover:border-primary/60 hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                    disabled={entry.graphicsValid === false}
                    title={`Tile ${entry.id} · CV5 group ${entry.group ?? "—"} · variant ${entry.variant ?? "—"}`}
                    aria-label={`${entry.name}, 그룹 ${entry.group ?? "알 수 없음"}, 변형 ${entry.variant ?? "알 수 없음"} 프롬프트에 추가`}
                    onClick={() => onMention(entry, layer, mentionKind)}
                  >
                    <PaletteThumbnail sessionId={sessionId} layer={layer} id={thumbnailId} />
                    <span className="max-w-full truncate font-mono text-[11px] font-semibold text-foreground">
                      #{entry.id}
                    </span>
                    <span className="font-mono text-[9px] text-muted-foreground">
                      G{entry.group ?? "—"} · V{entry.variant ?? "—"}
                    </span>
                    <span className="pointer-events-none absolute right-1 top-1 flex size-5 items-center justify-center rounded-full bg-background/85 text-primary shadow-sm">
                      <Plus className="size-3" aria-hidden="true" />
                    </span>
                  </button>
                );
              }
              return (
                <article key={entry.id} className="flex gap-2 rounded-lg border border-border bg-background/40 p-2">
                  <PaletteThumbnail sessionId={sessionId} layer={layer} id={thumbnailId} />
                  <div className="min-w-0 flex-1">
                    <h3 className="truncate text-xs font-medium" title={entry.name}>{entry.name}</h3>
                    {layer === "terrain" ? (
                      <p className="mt-1 font-mono text-[10px] leading-relaxed text-muted-foreground">
                        Brush {entry.id} · preview {entry.previewTile ?? "—"}
                        <br />
                        T{entry.terrainType ?? "—"} · H{entry.groundHeight ?? "—"} · B
                        {entry.buildability ?? "—"} · {entry.walkability ?? "unknown"}
                        <br />
                        VF4 {entry.ramp ? "ramp" : "flat"} ·
                        {entry.blocksView ? " blocks-view" : " open-view"} · high
                        {entry.highMinitiles ?? 0}/mid{entry.midMinitiles ?? 0}
                        <br />
                        graphics {entry.graphicsValid === false ? "invalid" : "valid"}
                      </p>
                    ) : (
                      <p className="mt-1 text-[10px] text-muted-foreground">
                        ID {entry.id}
                        {entry.width && entry.height ? ` · ${entry.width}×${entry.height}` : ""}
                        {entry.overlay ? " · terrain+sprite" : ""}
                      </p>
                    )}
                  </div>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="min-h-11 min-w-11"
                    disabled={entry.graphicsValid === false}
                    title={
                      entry.graphicsValid === false
                        ? "유효한 그래픽이 없어 이 항목을 사용할 수 없습니다."
                        : undefined
                    }
                    aria-label={`${entry.name} 프롬프트에 추가`}
                    onClick={() => onMention(entry, layer, mentionKind)}
                  >
                    <Plus className="size-4" aria-hidden="true" />
                  </Button>
                </article>
              );
            })}
            {total > 100 && (
              <nav
                className={cn(
                  "sticky bottom-0 flex items-center justify-between gap-2 rounded-md border border-border bg-card/95 p-2",
                  exactTerrain && "col-span-full",
                )}
                aria-label="팔레트 페이지"
              >
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={offset === 0 || loading}
                  onClick={() => setOffset((value) => Math.max(0, value - 100))}
                >
                  이전
                </Button>
                <span className="text-[10px] text-muted-foreground">
                  {Math.min(offset + 1, total)}–{Math.min(offset + entries.length, total)} / {total}
                </span>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={offset + entries.length >= total || loading}
                  onClick={() => setOffset((value) => value + 100)}
                >
                  다음
                </Button>
              </nav>
            )}
          </div>
        )}
      </div>
      <p className="border-t border-border p-2 text-[10px] leading-relaxed text-muted-foreground">
        영역 스탬프는 현재 후보, 가져온 영역은 pinned 외부 맵 스냅샷을 정확히 배치합니다. 둘 다 destination 권한을 만들지 않습니다.
      </p>
    </aside>
  );
}
