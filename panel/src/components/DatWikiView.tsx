/**
 * The DAT wiki — the whole version-matched StarCraft catalog, laid out the way
 * EUD Editor 3's DAT Editor lays it out: objects on the left, the selected
 * object's every property on the right.
 *
 * Read-only on purpose. `dat/*.json` holds the project's sparse overrides and
 * the agent writes them through `dat_patch`; this view shows the stock value
 * and, when an override exists, what the project runs instead. Nothing here can
 * create one, so no row can go around the `before == stock` rule the build
 * enforces.
 *
 * A number is shown as what it MEANS. `reference` on a field says which catalog
 * it indexes, so `Graphics 78` reads "78 · Marine" with a click through to that
 * flingy; a `text` reference is a ONE-based `stat_txt` id, so its string is the
 * `tbl` table's object `value - 1`; `flags` gives one label per bit, listed set
 * first. A field the catalog cannot name (an icon frame, an iscript id) keeps
 * its number rather than being guessed at.
 *
 * Pictures come from the installed StarCraft, the same way EUD Editor 3's DAT
 * Editor gets them: the object list draws each row's command icon (or
 * wireframe) out of one cached grid image, a field whose value is a frame
 * number draws that frame beside it, and a table that reaches a GRP draws the
 * object's own graphic. Without a resolvable install `schema.pictures` is
 * false and the view is text-only, with the reason stated once.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Check, Database, ImageOff, RefreshCw, Search, Square } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { DatWikiSheetFrame } from "@/components/DatWikiSheetFrame";
import {
  datWikiGraphic,
  datWikiObject,
  type DatWikiField,
  type DatWikiGraphic,
  type DatWikiObject,
  type DatWikiObjectValues,
  type DatWikiSchema,
  type DatWikiTable,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** One object row's height in the virtual list, in pixels. */
const ROW_HEIGHT = 34;
/** Rows rendered above and below the viewport so scrolling never shows a gap. */
const OVERSCAN = 8;
/** The longest edge of a list row's thumbnail, in pixels. */
const THUMBNAIL = 24;
/** The longest edge of a picture drawn beside a field's value. */
const INLINE_PICTURE = 28;
/** How far the object's own graphic may be enlarged; StarCraft art is small. */
const GRAPHIC_SCALE = 2;
/** The longest edge the enlarged graphic may reach, in pixels. */
const GRAPHIC_MAX = 160;

export interface DatWikiFocus {
  table: string;
  objectId: number;
  /** Bumped per request so re-opening the same object still reveals it. */
  nonce: number;
}

export interface DatWikiViewProps {
  schema: DatWikiSchema | null;
  loading: boolean;
  error: string | null;
  onRetry(): void;
  /** A sidebar (or cross-reference) request to reveal one object. */
  focus: DatWikiFocus | null;
}

/**
 * The object with this id. Catalog tables are dense (index === id), so the
 * direct hit is the common case; the search is what keeps a table that is not
 * dense from silently naming the wrong object.
 */
function objectAt(table: DatWikiTable | undefined, id: number): DatWikiObject | undefined {
  const direct = table?.objects[id];
  if (direct?.id === id) return direct;
  return table?.objects.find((object) => object.id === id);
}

function objectLabel(table: DatWikiTable | undefined, id: number): string {
  const name = objectAt(table, id)?.name;
  return name === undefined || name === "" ? `#${id}` : name;
}

/** The bits a flag value has set, as labels; unnamed bits keep their mask. */
function flagBits(value: number, labels: string[]): Array<{ label: string; on: boolean }> {
  return labels.map((label, bit) => ({
    label: label === "" ? `0x${(1 << bit).toString(16)}` : label,
    on: (value & (1 << bit)) !== 0,
  }));
}

function FlagList({ value, labels }: { value: number; labels: string[] }) {
  const bits = flagBits(value, labels);
  return (
    <ul className="grid max-w-3xl gap-x-6 gap-y-1 sm:grid-cols-2 lg:grid-cols-3">
      {bits.map((bit, index) => (
        <li
          key={`${bit.label}-${index}`}
          className={cn(
            "flex items-center gap-1.5 text-xs",
            bit.on ? "text-foreground" : "text-muted-foreground/70",
          )}
        >
          {bit.on ? (
            <Check className="size-3.5 shrink-0 text-primary" aria-hidden="true" />
          ) : (
            <Square className="size-3.5 shrink-0" aria-hidden="true" />
          )}
          <span className="min-w-0 truncate" title={bit.label}>
            {bit.label}
          </span>
          <span className="sr-only">{bit.on ? "켜짐" : "꺼짐"}</span>
        </li>
      ))}
    </ul>
  );
}

/** Enlarges a graphic without letting a tall one outgrow the header. */
function graphicScale(graphic: DatWikiGraphic): number {
  return Math.min(GRAPHIC_SCALE, GRAPHIC_MAX / Math.max(graphic.width, graphic.height));
}

/**
 * The picture a field's value is a frame of, when the schema says it is one.
 * A field carries its sheet, so this never has to guess from the field name.
 */
function FieldPicture({
  field,
  value,
  pictures,
}: {
  field: DatWikiField;
  value: number | string;
  pictures: boolean;
}) {
  if (!pictures || field.sheet === undefined || typeof value !== "number") return null;
  return (
    <DatWikiSheetFrame
      sheet={field.sheet}
      frame={value}
      size={INLINE_PICTURE}
      label={`${field.name} ${value}`}
      className="rounded-sm bg-muted/40"
    />
  );
}

/** One resolved value: the number plus what it points at, when it points. */
function ResolvedValue({
  value,
  field,
  schema,
  onOpen,
}: {
  value: number | string;
  field: DatWikiField;
  schema: DatWikiSchema;
  onOpen(table: string, objectId: number): void;
}) {
  if (typeof value === "string") {
    return (
      <code className="whitespace-pre-wrap break-all rounded bg-muted/60 px-1.5 py-0.5 font-mono text-xs">
        {value === "" ? "(빈 문자열)" : value}
      </code>
    );
  }
  if (field.flags !== undefined && field.flags.length > 0) {
    return (
      <div className="flex flex-col gap-1.5">
        <span className="font-mono text-xs text-muted-foreground">
          0x{value.toString(16)} ({value})
        </span>
        <FlagList value={value} labels={field.flags} />
      </div>
    );
  }
  const reference = field.reference;
  if (reference !== undefined && typeof reference === "object") {
    const target = schema.tables.find((table) => table.id === reference.table);
    return (
      <span className="flex flex-wrap items-baseline gap-1.5">
        <span className="font-mono tabular-nums">{value}</span>
        <Button
          type="button"
          size="sm"
          variant="link"
          className="h-auto p-0 text-xs"
          onClick={() => onOpen(reference.table, value)}
        >
          {objectLabel(target, value)}
        </Button>
        <span className="text-xs text-muted-foreground">{reference.table}</span>
      </span>
    );
  }
  if (reference === "text") {
    // A DAT label holds a ONE-based stat_txt id; the wiki's `tbl` table lists
    // the same strings zero-based, as `dat/tbl.json` addresses them.
    const tbl = schema.tables.find((table) => table.id === "tbl");
    const text = objectAt(tbl, value - 1)?.name;
    return (
      <span className="flex flex-wrap items-baseline gap-1.5">
        <span className="font-mono tabular-nums">{value}</span>
        {text !== undefined && (
          <Button
            type="button"
            size="sm"
            variant="link"
            className="h-auto p-0 text-xs"
            onClick={() => onOpen("tbl", value - 1)}
          >
            “{text}”
          </Button>
        )}
      </span>
    );
  }
  if (reference === "icon" || reference === "iscript") {
    return (
      <span className="flex flex-wrap items-baseline gap-1.5">
        <span className="font-mono tabular-nums">{value}</span>
        <span className="text-xs text-muted-foreground">
          {reference === "icon" ? "cmdicons 프레임" : "iscript 스크립트"}
        </span>
      </span>
    );
  }
  if (field.sheet !== undefined) {
    return (
      <span className="flex flex-wrap items-baseline gap-1.5">
        <span className="font-mono tabular-nums">{value}</span>
        <span className="text-xs text-muted-foreground">{field.sheet} 프레임</span>
      </span>
    );
  }
  return <span className="font-mono tabular-nums">{value}</span>;
}

export function DatWikiView({ schema, loading, error, onRetry, focus }: DatWikiViewProps) {
  const [tableId, setTableId] = useState<string | null>(null);
  const [objectId, setObjectId] = useState(0);
  const [query, setQuery] = useState("");
  const [values, setValues] = useState<DatWikiObjectValues | null>(null);
  const [valuesError, setValuesError] = useState<string | null>(null);
  const [valuesLoading, setValuesLoading] = useState(false);
  const [graphic, setGraphic] = useState<DatWikiGraphic | null>(null);

  const table = useMemo(
    () => schema?.tables.find((entry) => entry.id === tableId) ?? schema?.tables[0] ?? null,
    [schema, tableId],
  );

  // The first schema that arrives selects its first table, so the view is never
  // empty while a catalog is loaded.
  useEffect(() => {
    if (tableId === null && schema !== null && schema.tables.length > 0) {
      setTableId(schema.tables[0].id);
    }
  }, [schema, tableId]);

  const open = (nextTable: string, nextObject: number) => {
    setTableId(nextTable);
    setObjectId(nextObject);
    setQuery("");
  };

  // A sidebar request reveals its object; the nonce makes a repeat request a
  // change, so re-clicking the same row still scrolls back to it.
  const appliedFocus = useRef<number | null>(null);
  useEffect(() => {
    if (focus === null || appliedFocus.current === focus.nonce) return;
    appliedFocus.current = focus.nonce;
    setTableId(focus.table);
    setObjectId(focus.objectId);
    setQuery("");
  }, [focus]);

  // Load the selected object's values. A stale response never wins: only the
  // request that still matches the current selection is applied.
  useEffect(() => {
    if (table === null) return;
    let current = true;
    setValuesLoading(true);
    setValuesError(null);
    datWikiObject(table.id, objectId).then(
      (loaded) => {
        if (!current) return;
        setValues(loaded);
        setValuesLoading(false);
      },
      (reason) => {
        if (!current) return;
        setValues(null);
        setValuesError(
          `이 항목의 값을 읽지 못했습니다. 다른 항목을 골랐다가 다시 열어 보세요. (${String(reason)})`,
        );
        setValuesLoading(false);
      },
    );
    return () => {
      current = false;
    };
  }, [table, objectId]);

  // The object's own graphic. Only the tables that reach a GRP have one, and a
  // failure costs the picture alone — the properties are already on screen.
  const pictures = schema?.pictures ?? false;
  const drawsGraphic = pictures && (table?.graphic ?? false);
  useEffect(() => {
    if (table === null || !drawsGraphic) {
      setGraphic(null);
      return;
    }
    let current = true;
    setGraphic(null);
    datWikiGraphic(table.id, objectId).then(
      (loaded) => current && setGraphic(loaded),
      () => current && setGraphic(null),
    );
    return () => {
      current = false;
    };
  }, [table, objectId, drawsGraphic]);

  const trimmed = query.trim().toLowerCase();
  const objects = useMemo(() => {
    if (table === null) return [];
    if (trimmed === "") return table.objects;
    return table.objects.filter(
      (object) =>
        (object.name ?? "").toLowerCase().includes(trimmed) || String(object.id) === trimmed,
    );
  }, [table, trimmed]);

  // Virtualized object list: `sfxdata` alone holds 1144 rows and `tbl` 1547.
  const listRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(480);
  useEffect(() => {
    const element = listRef.current;
    if (element === null) return;
    const measure = () => setViewport(element.clientHeight);
    measure();
    // Feature-detected: the one measurement above is what a viewport-less
    // environment gets, and the list still renders from it.
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const first = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN);
  const last = Math.min(objects.length, Math.ceil((scrollTop + viewport) / ROW_HEIGHT) + OVERSCAN);
  const visible = objects.slice(first, last);

  // Keep the selected object in view when the selection moved (a focus request
  // or a cross-reference click), not while the user is scrolling.
  useEffect(() => {
    const element = listRef.current;
    if (element === null) return;
    const index = objects.findIndex((object) => object.id === objectId);
    if (index === -1) return;
    const top = index * ROW_HEIGHT;
    if (top < element.scrollTop || top + ROW_HEIGHT > element.scrollTop + element.clientHeight) {
      element.scrollTop = Math.max(0, top - element.clientHeight / 2);
    }
    // `objects` intentionally not a dependency: re-filtering must not scroll.
  }, [objectId, table]);

  if (loading) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
        <Spinner className="size-4" />
        DAT 카탈로그를 읽는 중입니다.
      </div>
    );
  }
  if (error !== null || schema === null || table === null) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
        <p className="max-w-md text-sm leading-relaxed text-muted-foreground">
          {error ?? "DAT 카탈로그를 읽지 못했습니다."}
        </p>
        <Button type="button" variant="outline" onClick={onRetry}>
          <RefreshCw className="size-4" aria-hidden="true" />
          다시 읽기
        </Button>
      </div>
    );
  }

  const fieldsByName = new Map(table.fields.map((field) => [field.name, field]));
  const selected = objectAt(table, objectId);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2">
        <Database className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
        <Select value={table.id} onValueChange={(next) => open(next, 0)}>
          <SelectTrigger className="h-8 w-56" aria-label="DAT 테이블">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {schema.tables.map((entry) => (
              <SelectItem key={entry.id} value={entry.id}>
                {entry.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className="relative min-w-0 flex-1 sm:max-w-xs">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            className="h-8 pl-8"
            value={query}
            placeholder={`${table.label} 안에서 검색`}
            aria-label="항목 검색"
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <span className="text-xs tabular-nums text-muted-foreground">
          {objects.length} / {table.objects.length}
        </span>
      </div>

      {schema.picturesNotice !== undefined && (
        <p className="flex items-start gap-2 border-b border-border/60 bg-muted/30 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
          <ImageOff className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
          {schema.picturesNotice}
          {/* The notice names an action, so the way back is right here. */}
          <Button
            type="button"
            size="sm"
            variant="link"
            className="h-auto shrink-0 p-0 text-xs"
            onClick={onRetry}
          >
            다시 읽기
          </Button>
        </p>
      )}

      {table.notice !== undefined && (
        <p className="border-b border-border/60 bg-muted/30 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
          {table.notice}
        </p>
      )}

      <div className="flex min-h-0 flex-1">
        {/* Object list */}
        <div
          ref={listRef}
          onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
          className="w-56 shrink-0 overflow-y-auto border-r border-border"
        >
          <div style={{ height: objects.length * ROW_HEIGHT, position: "relative" }}>
            <ul style={{ transform: `translateY(${first * ROW_HEIGHT}px)` }}>
              {visible.map((object) => (
                <li key={object.id} style={{ height: ROW_HEIGHT }}>
                  <button
                    type="button"
                    aria-current={object.id === objectId}
                    onClick={() => setObjectId(object.id)}
                    className={cn(
                      "flex h-full w-full items-center gap-2 px-3 text-left text-sm focus-visible:outline-none",
                      object.id === objectId
                        ? "bg-primary/10 font-medium text-foreground"
                        : "hover:bg-muted/60 focus-visible:bg-muted/60",
                    )}
                  >
                    {pictures && object.picture !== undefined && (
                      <DatWikiSheetFrame
                        sheet={object.picture.sheet}
                        frame={object.picture.frame}
                        size={THUMBNAIL}
                        label={objectLabel(table, object.id)}
                      />
                    )}
                    <span className="min-w-0 flex-1 truncate" title={objectLabel(table, object.id)}>
                      {objectLabel(table, object.id)}
                    </span>
                    <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
                      {object.id}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
          {objects.length === 0 && (
            <p className="p-3 text-xs text-muted-foreground">일치하는 항목이 없습니다.</p>
          )}
        </div>

        {/* Property table */}
        <div className="min-w-0 flex-1 overflow-y-auto">
          <div className="flex items-start gap-3 border-b border-border px-4 py-3">
            {pictures && selected?.picture !== undefined && (
              <DatWikiSheetFrame
                sheet={selected.picture.sheet}
                frame={selected.picture.frame}
                size={40}
                label={objectLabel(table, objectId)}
                className="mt-0.5 rounded bg-muted/40"
              />
            )}
            <div className="flex min-w-0 flex-wrap items-baseline gap-2">
              <h2 className="text-base font-semibold">{objectLabel(table, objectId)}</h2>
              <span className="font-mono text-xs text-muted-foreground">
                {table.id} #{objectId}
              </span>
            </div>
            {graphic !== null && (
              <figure className="ml-auto flex shrink-0 flex-col items-center gap-1">
                {/* Both edges are sized here: a CSS `auto` width drops the
                    image back to the GRP's own small pixel size. */}
                <img
                  src={graphic.png}
                  alt={`${objectLabel(table, objectId)}의 그래픽`}
                  width={Math.round(graphic.width * graphicScale(graphic))}
                  height={Math.round(graphic.height * graphicScale(graphic))}
                  className="rounded bg-muted/40"
                  style={{ imageRendering: "pixelated" }}
                />
                <figcaption
                  className="max-w-40 truncate font-mono text-[11px] text-muted-foreground"
                  title={graphic.grp}
                >
                  {graphic.grp}
                </figcaption>
              </figure>
            )}
          </div>

          {valuesLoading && (
            <div className="flex items-center gap-2 p-4 text-sm text-muted-foreground">
              <Spinner className="size-4" />
              값을 읽는 중입니다.
            </div>
          )}
          {valuesError !== null && (
            <p className="p-4 text-sm leading-relaxed text-muted-foreground">{valuesError}</p>
          )}

          {!valuesLoading && valuesError === null && values !== null && (
            <dl className="divide-y divide-border/60">
              {values.values.map((entry) => {
                const field = fieldsByName.get(entry.field);
                const overridden = entry.current !== undefined;
                return (
                  <div
                    key={entry.field}
                    className="flex flex-col gap-1 px-4 py-2.5 sm:flex-row sm:items-baseline sm:gap-4"
                  >
                    <dt className="flex shrink-0 items-baseline gap-2 sm:w-56">
                      <span className="text-sm font-medium">{entry.field}</span>
                      {field !== undefined && field.offset !== 0 && (
                        <span
                          className="font-mono text-[11px] text-muted-foreground"
                          title={`런타임 주소 · ${field.size}바이트 · ${field.min}~${field.max}`}
                        >
                          0x{field.offset.toString(16).toUpperCase()}
                        </span>
                      )}
                    </dt>
                    <dd className="flex min-w-0 flex-1 items-start gap-2 text-sm">
                      {field !== undefined && (
                        <FieldPicture
                          field={field}
                          value={(entry.current ?? entry.stock) as number | string}
                          pictures={pictures}
                        />
                      )}
                      {overridden ? (
                        <div className="flex flex-col gap-1">
                          <span className="flex flex-wrap items-center gap-2">
                            <Badge variant="secondary" className="shrink-0">
                              수정됨
                            </Badge>
                            {field !== undefined && (
                              <ResolvedValue
                                value={entry.current as number | string}
                                field={field}
                                schema={schema}
                                onOpen={open}
                              />
                            )}
                          </span>
                          <span className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                            원본
                            {field !== undefined && (
                              <ResolvedValue
                                value={entry.stock}
                                field={field}
                                schema={schema}
                                onOpen={open}
                              />
                            )}
                          </span>
                        </div>
                      ) : (
                        field !== undefined && (
                          <ResolvedValue
                            value={entry.stock}
                            field={field}
                            schema={schema}
                            onOpen={open}
                          />
                        )
                      )}
                    </dd>
                  </div>
                );
              })}
              {values.values.length === 0 && (
                <p className="p-4 text-sm text-muted-foreground">
                  이 항목은 표시할 속성이 없습니다.
                </p>
              )}
            </dl>
          )}
        </div>
      </div>
    </div>
  );
}
