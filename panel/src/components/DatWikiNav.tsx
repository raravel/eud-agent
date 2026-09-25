/**
 * The project sidebar's DAT wiki tab: a way INTO the catalog, not a viewer.
 *
 * Empty query -> the table list with its object counts. A query -> every object
 * whose name or id matches, across every table. Either way the click opens the
 * center "DAT 위키" tab on that object, which is where the values live: the
 * sidebar is too narrow for a 59-field object. A hit carries the same
 * thumbnail the center tab draws, out of the same cached sheet.
 */
import { useMemo, useState } from "react";
import { ChevronRight, Database, RefreshCw, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { DatWikiSheetFrame } from "@/components/DatWikiSheetFrame";
import type { DatWikiPicture, DatWikiSchema } from "@/lib/ipc";

/** Object hits shown for one query; a wider net would not fit the sidebar. */
const MAX_HITS = 80;
/** The longest edge of a hit's thumbnail, in pixels. */
const THUMBNAIL = 24;

export interface DatWikiNavProps {
  schema: DatWikiSchema | null;
  loading: boolean;
  error: string | null;
  onRetry(): void;
  /** Open the center tab on this object. */
  onOpen(table: string, objectId: number): void;
}

export function DatWikiNav({ schema, loading, error, onRetry, onOpen }: DatWikiNavProps) {
  const [query, setQuery] = useState("");
  const trimmed = query.trim().toLowerCase();
  const pictures = schema?.pictures ?? false;

  const hits = useMemo(() => {
    if (schema === null || trimmed === "") return [];
    const found: Array<{
      table: string;
      label: string;
      id: number;
      name: string;
      picture?: DatWikiPicture;
    }> = [];
    for (const table of schema.tables) {
      for (const object of table.objects) {
        const name = object.name ?? `#${object.id}`;
        if (
          name.toLowerCase().includes(trimmed) ||
          String(object.id) === trimmed ||
          table.id.toLowerCase().includes(trimmed)
        ) {
          found.push({
            table: table.id,
            label: table.label,
            id: object.id,
            name,
            picture: object.picture,
          });
          if (found.length >= MAX_HITS) return found;
        }
      }
    }
    return found;
  }, [schema, trimmed]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-border/60 p-3">
        <p className="mb-2 text-xs leading-relaxed text-muted-foreground">
          스타크래프트 원본 DAT 전체를 읽습니다. 프로젝트가 바꾼 값은 원본과 함께 표시됩니다.
        </p>
        <div className="relative">
          <Search
            className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            className="h-9 pl-8"
            value={query}
            placeholder="유닛 · 무기 · 오더 검색"
            aria-label="DAT 항목 검색"
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
      </div>

      {loading && (
        <div className="flex items-center gap-2 p-3 text-xs text-muted-foreground">
          <Spinner className="size-4" />
          카탈로그를 읽는 중입니다.
        </div>
      )}

      {error !== null && (
        <div className="flex flex-col items-start gap-2 p-3">
          <p className="text-xs leading-relaxed text-muted-foreground">{error}</p>
          <Button type="button" size="sm" variant="outline" onClick={onRetry}>
            <RefreshCw className="size-3.5" aria-hidden="true" />
            다시 읽기
          </Button>
        </div>
      )}

      {schema !== null && (
        <ul className="min-h-0 flex-1 overflow-y-auto">
          {trimmed === ""
            ? schema.tables.map((table) => (
                <li key={table.id}>
                  <button
                    type="button"
                    onClick={() => onOpen(table.id, table.objects[0]?.id ?? 0)}
                    className="flex min-h-11 w-full items-center gap-2 border-b border-border/60 px-3 py-2.5 text-left text-sm hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:outline-none"
                  >
                    <Database className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{table.label}</span>
                    <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
                      {table.objects.length}
                    </span>
                    <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                  </button>
                </li>
              ))
            : hits.map((hit) => (
                <li key={`${hit.table}:${hit.id}`}>
                  <button
                    type="button"
                    onClick={() => onOpen(hit.table, hit.id)}
                    className="flex min-h-11 w-full items-center gap-2 border-b border-border/60 px-3 py-2 text-left hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:outline-none"
                  >
                    {pictures && hit.picture !== undefined && (
                      <DatWikiSheetFrame
                        sheet={hit.picture.sheet}
                        frame={hit.picture.frame}
                        size={THUMBNAIL}
                        label={hit.name}
                      />
                    )}
                    <span className="flex min-w-0 flex-1 flex-col items-start gap-0.5">
                      <span className="w-full truncate text-sm">{hit.name}</span>
                      <span className="w-full truncate text-xs text-muted-foreground">
                        {hit.label} · #{hit.id}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
          {trimmed !== "" && hits.length === 0 && (
            <li className="p-3 text-xs text-muted-foreground">일치하는 항목이 없습니다.</li>
          )}
          {hits.length >= MAX_HITS && (
            <li className="p-3 text-xs text-muted-foreground">
              결과가 많습니다. 검색어를 더 좁혀 주세요.
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
