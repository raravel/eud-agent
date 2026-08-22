/**
 * Dat-edit WIKI view — a per-project last-value ledger of dat/xdat/tbl/req/btn
 * edits the agent applied via the DATA EDITOR and the user APPROVED.
 *
 * Rendered as a PERMANENT, drag-resizable LEFT SIDEBAR (App lays it out beside
 * the chat column). Navigation is a mobile-style one-column DRILLDOWN — never a
 * nested tree — across three depths that slide horizontally:
 *   depth 0  category   (table/dat group, e.g. "dat units", "btn")
 *   depth 1  item       (a single objId; units show their resolved name)
 *   depth 2  editor     (the item's "property = value" rows, inline-editable)
 * A back (‹) control pops one depth; Escape pops a depth (or closes at depth 0).
 *
 * Committing a value edit (Enter / blur with a changed value) calls `onSave`
 * with the FULL entries map (the changed entry patched); the core persists it
 * with `editedByUser=true` and pushes the refreshed ledger back. A may-differ
 * note heads the editor depth (the ledger is the agent's last applied value,
 * not a live read of the map).
 */
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronLeft, ChevronRight, Database, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { WikiState } from "@/state/store";
import type { LedgerEntry } from "@/lib/ipc";
import { cn } from "@/lib/utils";

/** What this wiki is for — shown atop the top-level (category) depth. */
const PURPOSE_NOTE =
  "에이전트가 데이터 에디터에서 수정해 승인된 dat 값을 기록합니다. 다음 작업의 참고용입니다.";

/** The may-differ note (mirrors the Rust `MAY_DIFFER_LABEL`). */
const MAY_DIFFER_NOTE =
  "에이전트가 마지막으로 적용한 값입니다. 실제 맵과 다를 수 있습니다.";

/** Resizable sidebar width bounds + persistence key. */
const MIN_WIDTH = 220;
const MAX_WIDTH = 560;
const DEFAULT_WIDTH = 300;
const WIDTH_KEY = "eud.wiki.sidebar.width";

export interface WikiViewProps {
  /** The current dat-edit ledger snapshot. */
  wiki: WikiState;
  onClose(): void;
  /** Fill a parent-owned project sidebar instead of owning left-side width. */
  embedded?: boolean;
  /** Persist the full (patched) entries map; the core flips editedByUser=true. */
  onSave(entries: Record<string, LedgerEntry>): void;
}

/** One item (a single objId within a table/dat group) and its property entries. */
interface ItemGroup {
  /** Stable item key (table:dat:objId) for React keying. */
  key: string;
  objId: number;
  /** Resolved unit name (table=dat & dat=units) or undefined. */
  itemName?: string;
  /** Ledger entries belonging to this item, with their map keys. */
  entries: Array<{ mapKey: string; entry: LedgerEntry }>;
}

/** One table/dat group (e.g. "dat units", "btn"). */
interface TableGroup {
  /** Stable group key (table + dat). */
  key: string;
  table: string;
  dat: string;
  /** Group heading: "{table} {dat}" or just "{table}" when dat is empty. */
  heading: string;
  items: ItemGroup[];
}

/** Stringify a ledger value for display in the input (numbers/strings only). */
function valueToText(value: number | string): string {
  return typeof value === "string" ? value : String(value);
}

/** Read the persisted sidebar width, clamped; falls back to the default. */
function readStoredWidth(): number {
  if (typeof localStorage === "undefined") return DEFAULT_WIDTH;
  const saved = Number(localStorage.getItem(WIDTH_KEY));
  return Number.isFinite(saved) && saved >= MIN_WIDTH && saved <= MAX_WIDTH
    ? saved
    : DEFAULT_WIDTH;
}

/**
 * Group the flat ledger into table/dat -> item -> properties. Insertion order
 * follows the (already key-sorted) entries map, so the rendered tree is
 * deterministic without re-sorting.
 */
function buildGroups(entries: Record<string, LedgerEntry>): TableGroup[] {
  const tables = new Map<string, TableGroup>();
  for (const [mapKey, entry] of Object.entries(entries)) {
    const tableKey = `${entry.table} ${entry.dat}`;
    let group = tables.get(tableKey);
    if (group === undefined) {
      group = {
        key: tableKey,
        table: entry.table,
        dat: entry.dat,
        heading: entry.dat ? `${entry.table} ${entry.dat}` : entry.table,
        items: [],
      };
      tables.set(tableKey, group);
    }
    const itemKey = `${tableKey} ${entry.objId}`;
    let item = group.items.find((it) => it.key === itemKey);
    if (item === undefined) {
      item = { key: itemKey, objId: entry.objId, itemName: entry.itemName, entries: [] };
      group.items.push(item);
    }
    if (item.itemName === undefined && entry.itemName !== undefined) {
      item.itemName = entry.itemName;
    }
    item.entries.push({ mapKey, entry });
  }
  return [...tables.values()];
}

/** A small uppercase table-family badge (dat / xdat / tbl / req / btn). */
function TableBadge({ table }: { table: string }) {
  return (
    <span className="rounded bg-sky-500/15 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-sky-400">
      {table}
    </span>
  );
}

/** One drilldown row: label (+ optional count) with a forward chevron. */
function NavRow({
  label,
  count,
  onClick,
}: {
  label: ReactNode;
  count: number;
  onClick(): void;
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className="flex min-h-11 w-full items-center gap-2 border-b border-border/60 px-3 py-2.5 text-left text-sm hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:outline-none"
      >
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <span className="shrink-0 text-xs tabular-nums text-muted-foreground">
          {count}
        </span>
        <ChevronRight
          className="size-4 shrink-0 text-muted-foreground"
          aria-hidden="true"
        />
      </button>
    </li>
  );
}

/** One sliding depth column (1/3 of the 300%-wide track = the sidebar width). */
function DepthPanel({
  hidden,
  children,
}: {
  hidden: boolean;
  children: ReactNode;
}) {
  return (
    <div
      aria-hidden={hidden}
      inert={hidden}
      className="h-full w-1/3 shrink-0 overflow-y-auto"
    >
      {children}
    </div>
  );
}

/** A single inline-editable "property = value" row. */
function PropertyRow({
  mapKey,
  entry,
  onCommit,
}: {
  mapKey: string;
  entry: LedgerEntry;
  onCommit(mapKey: string, raw: string): void;
}) {
  const original = valueToText(entry.value);
  const [draft, setDraft] = useState(original);
  // Keep the draft in sync when the underlying value changes (server push).
  const [seen, setSeen] = useState(original);
  if (seen !== original) {
    setSeen(original);
    setDraft(original);
  }

  const commit = () => {
    if (draft !== original) onCommit(mapKey, draft);
  };

  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span className="font-medium">{entry.property}</span>
      <span className="text-muted-foreground">=</span>
      <Input
        className="h-7 w-40 px-2 py-0.5 text-sm"
        value={draft}
        aria-label={`${entry.property} 값`}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commit();
          } else if (event.key === "Escape") {
            setDraft(original);
          }
        }}
      />
      {entry.editedByUser && (
        <span className="text-xs text-muted-foreground">사용자 수정</span>
      )}
    </div>
  );
}

export function WikiView({ wiki, onClose, onSave, embedded = false }: WikiViewProps) {
  const groups = useMemo(() => buildGroups(wiki.entries), [wiki.entries]);

  // Drilldown selection. depth derives from what is still resolvable, so a
  // server push that removes the selected group/item auto-pops to a valid depth.
  const [catKey, setCatKey] = useState<string | null>(null);
  const [itemKey, setItemKey] = useState<string | null>(null);
  const selectedGroup = useMemo(
    () => groups.find((g) => g.key === catKey) ?? null,
    [groups, catKey],
  );
  const selectedItem = useMemo(
    () => selectedGroup?.items.find((it) => it.key === itemKey) ?? null,
    [selectedGroup, itemKey],
  );
  const depth = selectedItem ? 2 : selectedGroup ? 1 : 0;

  // Drop a stale selection when a push removes it (keeps catKey/itemKey honest).
  useEffect(() => {
    if (catKey && !selectedGroup) {
      setCatKey(null);
      setItemKey(null);
    }
  }, [catKey, selectedGroup]);
  useEffect(() => {
    if (itemKey && !selectedItem) setItemKey(null);
  }, [itemKey, selectedItem]);

  const back = () => {
    if (selectedItem) setItemKey(null);
    else if (selectedGroup) setCatKey(null);
  };

  // Resizable width (persisted across opens/sessions).
  const [width, setWidth] = useState(readStoredWidth);
  useEffect(() => {
    try {
      localStorage.setItem(WIDTH_KEY, String(width));
    } catch {
      /* storage unavailable — width stays session-local */
    }
  }, [width]);
  const dragRef = useRef<{ startX: number; startW: number } | null>(null);

  // Commit one property edit: patch the changed entry's value into the full
  // entries map and hand the whole map to onSave (the core flips editedByUser).
  // A numeric original coerces a numeric-looking edit back to a number so the
  // ledger value type stays consistent; otherwise the raw string is kept.
  const handleCommit = (mapKey: string, raw: string) => {
    const current = wiki.entries[mapKey];
    if (current === undefined) return;
    let value: number | string = raw;
    if (typeof current.value === "number") {
      const num = Number(raw);
      if (raw.trim() !== "" && Number.isFinite(num)) value = num;
    }
    const patched: LedgerEntry = { ...current, value, editedByUser: true };
    onSave({ ...wiki.entries, [mapKey]: patched });
  };

  const title =
    depth === 2 && selectedItem
      ? (selectedItem.itemName ?? `#${selectedItem.objId}`)
      : depth === 1 && selectedGroup
        ? selectedGroup.heading
        : "dat 편집 위키";

  return (
    <aside
      aria-label="dat 편집 위키"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        if (depth > 0) back();
        else onClose();
      }}
      style={embedded ? undefined : { width }}
      className={cn(
        "relative flex h-full min-w-0 flex-col bg-card/40",
        embedded ? "w-full flex-1" : "shrink-0 border-r border-border",
      )}
    >
      {/* Drilldown nav bar: back (or wiki glyph) + current title + close. */}
      <div className="flex items-center gap-1 border-b border-border px-2 py-2">
        {depth > 0 ? (
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="뒤로"
            onClick={back}
          >
            <ChevronLeft className="size-4" aria-hidden="true" />
          </Button>
        ) : (
          <span className="inline-flex size-9 shrink-0 items-center justify-center">
            <Database
              className="size-4 text-muted-foreground"
              aria-hidden="true"
            />
          </span>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold" title={title}>
          {title}
        </h2>
        {!embedded && (
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label="닫기"
            onClick={onClose}
          >
            <X className="size-4" aria-hidden="true" />
          </Button>
        )}
      </div>

      {/* Sliding 3-depth track (forward = slide left, back = slide right). */}
      <div className="relative min-h-0 flex-1 overflow-hidden">
        <div
          className="flex h-full transition-transform duration-200 ease-out motion-reduce:transition-none"
          style={{ width: "300%", transform: `translateX(-${(depth * 100) / 3}%)` }}
        >
          {/* depth 0 — categories */}
          <DepthPanel hidden={depth !== 0}>
            <p className="border-b border-border/60 px-3 py-2.5 text-xs leading-relaxed text-muted-foreground">
              {PURPOSE_NOTE}
            </p>
            {groups.length === 0 ? (
              <p className="p-3 text-xs text-muted-foreground">
                기록된 dat 편집이 없습니다.
              </p>
            ) : (
              <ul className="flex flex-col">
                {groups.map((group) => (
                  <NavRow
                    key={group.key}
                    count={group.items.length}
                    onClick={() => setCatKey(group.key)}
                    label={
                      <span className="flex items-center gap-1.5">
                        <TableBadge table={group.table} />
                        {group.dat && (
                          <span className="truncate font-medium">{group.dat}</span>
                        )}
                      </span>
                    }
                  />
                ))}
              </ul>
            )}
          </DepthPanel>

          {/* depth 1 — items */}
          <DepthPanel hidden={depth !== 1}>
            {selectedGroup && (
              <ul className="flex flex-col">
                {selectedGroup.items.map((item) => (
                  <NavRow
                    key={item.key}
                    count={item.entries.length}
                    onClick={() => setItemKey(item.key)}
                    label={
                      <span
                        className={cn(
                          "truncate",
                          item.itemName
                            ? "font-medium"
                            : "text-muted-foreground",
                        )}
                      >
                        {item.itemName ?? `#${item.objId}`}
                      </span>
                    }
                  />
                ))}
              </ul>
            )}
          </DepthPanel>

          {/* depth 2 — property editor */}
          <DepthPanel hidden={depth !== 2}>
            {selectedItem && (
              <div className="flex flex-col gap-2 p-3">
                <p className="text-xs text-muted-foreground">{MAY_DIFFER_NOTE}</p>
                {selectedItem.entries.map(({ mapKey, entry }) => (
                  <PropertyRow
                    key={mapKey}
                    mapKey={mapKey}
                    entry={entry}
                    onCommit={handleCommit}
                  />
                ))}
              </div>
            )}
          </DepthPanel>
        </div>
      </div>

      {!embedded && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="사이드바 너비 조절"
          onPointerDown={(event) => {
            dragRef.current = { startX: event.clientX, startW: width };
            event.currentTarget.setPointerCapture(event.pointerId);
            event.preventDefault();
          }}
          onPointerMove={(event) => {
            const drag = dragRef.current;
            if (!drag) return;
            const next = Math.min(
              MAX_WIDTH,
              Math.max(MIN_WIDTH, drag.startW + (event.clientX - drag.startX)),
            );
            setWidth(next);
          }}
          onPointerUp={(event) => {
            if (!dragRef.current) return;
            dragRef.current = null;
            event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          className="absolute inset-y-0 right-0 z-10 w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/40 active:bg-primary/60"
        />
      )}
    </aside>
  );
}
