import { useTranslation } from "react-i18next";
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
import { Checkbox } from "@/components/ui/checkbox";
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
  { layer: "terrain", label: "map.layers.terrain", icon: Mountain },
  { layer: "buildings", label: "map.layers.buildings", icon: Building2 },
  { layer: "units", label: "map.layers.units", icon: Users },
  { layer: "doodads", label: "map.layers.doodads", icon: Layers3 },
  { layer: "sprites", label: "map.layers.sprites", icon: Sparkles },
  { layer: "locations", label: "map.layers.locations", icon: MapPinPlus },
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
  const { t } = useTranslation();
  const [layer, setLayer] = useState<MapLayer>("terrain");
  const [terrainMode, setTerrainMode] = useState<"tiles" | "brushes">("tiles");
  const [hideNullTiles, setHideNullTiles] = useState(false);
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
      void mapCatalog({
        sessionId,
        kind,
        query,
        offset,
        limit: 100,
        hideNullTiles: kind === "tiles" && hideNullTiles,
      })
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
  }, [hideNullTiles, kind, layer, offset, query, sessionId]);

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
        <div className="grid grid-cols-3 gap-1" role="tablist" aria-label={t("map.palette.layersAria")}>
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
                {t(tab.label)}
              </button>
            );
          })}
        </div>
      </div>

      <div className="space-y-2 border-b border-border p-3">
        <label className="relative block">
          <span className="sr-only">{t("map.palette.search")}</span>
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
            placeholder={t("map.palette.searchPlaceholder")}
          />
        </label>
        {layer === "terrain" && (
          <div
            className="flex min-h-11 flex-wrap items-center gap-1"
            role="group"
            aria-label={t("map.palette.terrainModeAria")}
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
              {t("map.palette.tiles")}
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
              {t("map.palette.brushes")}
            </Button>
            <span className="ml-auto text-[10px] text-muted-foreground">{tileset}</span>
          </div>
        )}
        {exactTerrain && (
          <div className="flex min-h-11 items-center gap-2 text-xs">
            <Checkbox
              id="palette-hide-null-tiles"
              checked={hideNullTiles}
              onCheckedChange={(checked) => {
                setHideNullTiles(checked === true);
                setOffset(0);
                setEntries([]);
                setTotal(0);
              }}
            />
            <label htmlFor="palette-hide-null-tiles">{t("map.palette.hideNull")}</label>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        <section className="mb-2 space-y-2 rounded-lg border border-primary/25 bg-primary/5 p-2" aria-label={t("map.palette.stamps")}>
          <div className="flex items-center gap-2 px-1">
            <CopyPlus className="size-4 text-primary" aria-hidden="true" />
            <h2 className="text-xs font-semibold text-foreground">{t("map.palette.stamps")}</h2>
            <span className="ml-auto text-[10px] text-muted-foreground">{t("map.palette.currentCandidate")}</span>
          </div>
          {filteredSelections.length === 0 ? (
            <p className="rounded-md border border-dashed border-border px-3 py-4 text-center text-[11px] leading-relaxed text-muted-foreground">
              {t("map.palette.noStamps")}
            </p>
          ) : (
            filteredSelections.map((selection) => {
              const width = selection.bounds.right - selection.bounds.left;
              const height = selection.bounds.bottom - selection.bounds.top;
              const layerLabel =
                selection.layers.length === 0 ? t("map.allLayers") : selection.layers.join(" · ");
              return (
                <article key={selection.id} className="rounded-md border border-border bg-background/60 p-2">
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <h3 className="truncate text-xs font-medium" title={selection.label}>
                        {selection.label}
                      </h3>
                      <p className="mt-1 font-mono text-[10px] text-muted-foreground">
                        {width}×{height} · {t("map.cells", { count: selection.selectedCells, n: selection.selectedCells.toLocaleString() })}
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
                      aria-label={t("map.palette.stampPlace", { label: selection.label })}
                      onClick={() => onStampPlace(selection)}
                    >
                      <CopyPlus className="size-4" aria-hidden="true" />
                      {t("map.palette.stampPlaceButton")}
                    </Button>
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      className="min-h-11 min-w-11"
                      aria-label={t("map.palette.stampMention", { label: selection.label })}
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
        <section className="mb-2 space-y-2 rounded-lg border border-cyan-500/25 bg-cyan-500/5 p-2" aria-label={t("map.palette.imported")}>
          <div className="flex items-center gap-2 px-1">
            <CopyPlus className="size-4 text-cyan-400" aria-hidden="true" />
            <h2 className="text-xs font-semibold text-foreground">{t("map.palette.imported")}</h2>
            <span className="ml-auto text-[10px] text-muted-foreground">{t("map.palette.pinnedSnapshot")}</span>
          </div>
          {filteredImported.length === 0 ? (
            <p className="rounded-md border border-dashed border-border px-3 py-4 text-center text-[11px] leading-relaxed text-muted-foreground">
              {t("map.palette.noImported")}
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
                        {width}×{height} · {t("map.cells", { count: stamp.selectedCells, n: stamp.selectedCells.toLocaleString() })}
                      </p>
                      <p className="mt-1 truncate text-[10px] text-muted-foreground">
                        {stamp.layers.length === 0 ? t("map.allLayers") : stamp.layers.join(" · ")}
                      </p>
                      {!enabled && (
                        <p className="mt-1 text-[10px] text-destructive">
                          {stamp.unavailableReason ?? t("map.palette.incompatible")}
                        </p>
                      )}
                    </div>
                  </div>
                  <div className="mt-2 flex justify-end gap-1">
                    <Button type="button" size="sm" variant="secondary" disabled={!enabled} onClick={() => onImportedPlace(stamp)}>
                      <CopyPlus className="size-4" aria-hidden="true" />
                      {t("map.palette.stampPlaceButton")}
                    </Button>
                    <Button type="button" size="sm" variant="outline" disabled={!enabled} onClick={() => onImportedMention(stamp)}>
                      <Plus className="size-4" aria-hidden="true" />
                      {t("map.palette.addMention")}
                    </Button>
                    <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label={t("map.palette.deleteImported", { label: stamp.label })} onClick={() => onImportedDelete(stamp)}>
                      <Trash2 className="size-4" aria-hidden="true" />
                    </Button>
                  </div>
                </article>
              );
            })
          )}
        </section>
        {error && <p role="alert" className="m-2 rounded-md bg-destructive/15 p-2 text-xs text-destructive-foreground">{error}</p>}
        {loading && <div className="flex items-center gap-2 p-3 text-xs text-muted-foreground"><Spinner />{t("map.palette.loading")}</div>}
        {layer === "locations" ? (
          <div className="space-y-2">
            <Button type="button" variant="outline" className="h-12 w-full justify-start" onClick={onNewLocation}>
              <MapPinPlus className="size-4" aria-hidden="true" />
              {t("map.palette.newLocation")}
            </Button>
            {filteredLocations.map((location) => (
              <button
                key={location.id}
                type="button"
                className="flex min-h-12 w-full items-center gap-2 rounded-md border border-border bg-background/40 px-3 text-left text-xs hover:bg-muted"
                onClick={() => onLocation(location)}
              >
                <span className="font-mono text-cyan-300">#{location.id}</span>
                <span className="min-w-0 flex-1 truncate">{location.name || t("map.mention.noName")}</span>
                {location.anywhere && <span className="text-[10px] text-amber-300">{t("map.mention.readOnly")}</span>}
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
                    aria-label={t("map.palette.tileAria", { name: entry.name, group: entry.group ?? t("map.palette.unknown"), variant: entry.variant ?? t("map.palette.unknown") })}
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
                        ? t("map.palette.noGraphic")
                        : undefined
                    }
                    aria-label={t("map.palette.addToPrompt", { name: entry.name })}
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
                aria-label={t("map.palette.pageAria")}
              >
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={offset === 0 || loading}
                  onClick={() => setOffset((value) => Math.max(0, value - 100))}
                >
                  {t("map.palette.prev")}
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
                  {t("map.palette.next")}
                </Button>
              </nav>
            )}
          </div>
        )}
      </div>
      <p className="border-t border-border p-2 text-[10px] leading-relaxed text-muted-foreground">
        {t("map.palette.footnote")}
      </p>
    </aside>
  );
}
