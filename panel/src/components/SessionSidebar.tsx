import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  LoaderCircle,
  MessageCircleQuestion,
  Pencil,
  Plus,
  Search,
  Trash2,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { PROVIDER_LABELS } from "@/providers/providerCopy";
import type { ProviderId } from "@/providers/types";

export type SessionActivity =
  | "idle"
  | "running_read"
  | "waiting_input"
  | "running_write"
  | "review"
  | "error";

export interface SessionSidebarRow {
  id: string;
  name: string;
  lastConversationAt: number;
  provider: ProviderId;
  activity: SessionActivity;
  persisted: boolean;
}

export interface SessionSidebarProps {
  project: string;
  rows: SessionSidebarRow[];
  selectedId: string | null;
  collapsed: boolean;
  onCollapsedChange(collapsed: boolean): void;
  onNew(): void;
  onSelect(id: string): void;
  onRename(id: string, name: string): void;
  onDelete(id: string): void;
}

const WIDTH_KEY = "eud.session-sidebar.width";
const DEFAULT_WIDTH = 272;
const MIN_WIDTH = 220;
const MAX_WIDTH = 420;
const COLLAPSED_WIDTH = 56;

function readStoredWidth(): number {
  if (typeof localStorage === "undefined") return DEFAULT_WIDTH;
  const stored = localStorage.getItem(WIDTH_KEY);
  if (stored === null) return DEFAULT_WIDTH;
  const saved = Number(stored);
  return Number.isFinite(saved)
    ? Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, saved))
    : DEFAULT_WIDTH;
}

function activityLabel(row: SessionSidebarRow): string {
  switch (row.activity) {
    case "running_read":
      return "분석 중";
    case "waiting_input":
      return "응답 필요";
    case "running_write":
      return "변경 중";
    case "review":
      return "검토 필요";
    case "error":
      return "오류";
    default:
      return row.persisted ? "유휴" : "새 세션";
  }
}

function ActivityIcon({ activity }: { activity: SessionActivity }) {
  switch (activity) {
    case "running_read":
    case "running_write":
      return (
        <LoaderCircle
          className="size-3.5 animate-spin text-primary motion-reduce:animate-none"
          aria-hidden="true"
        />
      );
    case "waiting_input":
      return (
        <MessageCircleQuestion
          className="size-3.5 text-amber-400"
          aria-hidden="true"
        />
      );
    case "review":
      return <AlertCircle className="size-3.5 text-amber-400" aria-hidden="true" />;
    case "error":
      return <AlertCircle className="size-3.5 text-destructive" aria-hidden="true" />;
    default:
      return <CheckCircle2 className="size-3.5 text-muted-foreground" aria-hidden="true" />;
  }
}

function formatLastConversation(lastConversationAt: number): string {
  if (!Number.isFinite(lastConversationAt) || lastConversationAt <= 0) return "";
  const date = new Date(lastConversationAt);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export function SessionSidebar({
  project,
  rows,
  selectedId,
  collapsed,
  onCollapsedChange,
  onNew,
  onSelect,
  onRename,
  onDelete,
}: SessionSidebarProps) {
  const [query, setQuery] = useState("");
  const [width, setWidth] = useState(readStoredWidth);
  const [dragging, setDragging] = useState(false);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    try {
      localStorage.setItem(WIDTH_KEY, String(width));
    } catch {
      // Width persistence is optional; current-window resizing still works.
    }
  }, [width]);

  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return rows;
    return rows.filter((row) => row.name.toLocaleLowerCase().includes(normalized));
  }, [query, rows]);

  const handleRename = (row: SessionSidebarRow) => {
    const next = window.prompt("새 이름을 입력하세요.", row.name)?.trim();
    if (next && next !== row.name) onRename(row.id, next);
  };

  const handleDelete = (row: SessionSidebarRow) => {
    if (
      row.activity === "running_read" ||
      row.activity === "running_write" ||
      row.activity === "waiting_input" ||
      row.activity === "review"
    )
      return;
    if (window.confirm(`'${row.name}' 대화를 삭제할까요?`)) onDelete(row.id);
  };

  return (
    <aside
      aria-label="세션"
      style={{ width: collapsed ? COLLAPSED_WIDTH : width }}
      className={cn(
        "relative flex h-full min-w-0 shrink-0 flex-col overflow-x-hidden border-r border-border bg-card/25",
        !dragging && "transition-[width] duration-200 motion-reduce:transition-none",
      )}
    >
      <div
        className={cn(
          "flex min-h-14 items-center border-b border-border/80 bg-card/35",
          collapsed ? "justify-center px-1" : "gap-2 px-3",
        )}
      >
        {!collapsed && (
          <div className="min-w-0 flex-1 overflow-hidden">
            <div className="flex min-w-0 items-center gap-2">
              <h2 className="truncate text-sm font-semibold">세션</h2>
              <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-[10px] tabular-nums text-muted-foreground">
                {rows.length}
              </span>
            </div>
            <p
              className="truncate text-[11px] leading-4 text-muted-foreground"
              title={project}
            >
              {project || "프로젝트 없음"}
            </p>
          </div>
        )}
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="size-11 shrink-0"
          aria-label={collapsed ? "세션 사이드바 펼치기" : "세션 사이드바 접기"}
          onClick={() => onCollapsedChange(!collapsed)}
        >
          {collapsed ? (
            <ChevronRight className="size-4" aria-hidden="true" />
          ) : (
            <ChevronLeft className="size-4" aria-hidden="true" />
          )}
        </Button>
      </div>

      <div
        className={cn(
          "border-b border-border/70",
          collapsed ? "p-1.5" : "space-y-2 p-2.5",
        )}
      >
        <Button
          type="button"
          variant="outline"
          size={collapsed ? "icon" : "sm"}
          className={cn(
            "border-primary/20 bg-primary/5 hover:border-primary/40 hover:bg-primary/10",
            collapsed ? "size-11" : "h-9 w-full justify-start gap-2 px-2.5",
          )}
          aria-label="새 세션"
          onClick={onNew}
        >
          <Plus className="size-4 text-primary" aria-hidden="true" />
          {!collapsed && <span className="font-medium">새 세션</span>}
        </Button>
        {!collapsed && (
          <label className="relative block min-w-0">
            <Search
              className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <span className="sr-only">세션 검색</span>
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="h-9 min-w-0 bg-background/45 pl-8 text-xs"
              placeholder="세션 검색"
            />
          </label>
        )}
      </div>

      {!collapsed && (
        <div className="flex min-w-0 items-center justify-between gap-2 px-3 pb-1 pt-2.5 text-[10px] font-medium uppercase tracking-[0.08em] text-muted-foreground">
          <span className="truncate">현재 프로젝트</span>
          <span className="shrink-0 tabular-nums">{filtered.length}</span>
        </div>
      )}

      <nav
        aria-label="현재 프로젝트 세션"
        className="min-h-0 min-w-0 flex-1 overflow-x-hidden overflow-y-auto p-1.5"
      >
        {filtered.length === 0 ? (
          !collapsed && (
            <div className="mx-1 mt-1 rounded-md border border-dashed border-border/70 px-3 py-6 text-center">
              <p className="text-xs font-medium text-muted-foreground">
                표시할 세션이 없습니다
              </p>
              <p className="mt-1 text-[11px] leading-4 text-muted-foreground/70">
                새 세션에서 작업을 시작하세요.
              </p>
            </div>
          )
        ) : (
          <ul className="grid min-w-0 max-w-full gap-1 overflow-x-hidden">
            {filtered.map((row) => {
              const selected = row.id === selectedId;
              const label = activityLabel(row);
              const lastConversation = formatLastConversation(row.lastConversationAt);
              return (
                <li key={row.id} className="group relative min-w-0 max-w-full overflow-hidden">
                  <button
                    type="button"
                    aria-current={selected ? "page" : undefined}
                    aria-label={`${row.name}, ${label}`}
                    title={`${row.name} · ${label}`}
                    onClick={() => onSelect(row.id)}
                    className={cn(
                      "flex max-w-full overflow-hidden rounded-md text-left outline-none transition-colors focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                      collapsed
                        ? "size-11 items-center justify-center"
                        : "min-h-[58px] w-full min-w-0 items-start gap-2.5 px-2.5 py-2 pr-[5.5rem]",
                      selected
                        ? "bg-primary/10 text-foreground shadow-[inset_2px_0_0_var(--primary)]"
                        : "text-muted-foreground hover:bg-muted/65 hover:text-foreground",
                    )}
                  >
                    {collapsed ? (
                      <span className="relative flex size-8 items-center justify-center rounded-md border border-border/70 bg-muted/45 text-xs font-semibold text-foreground">
                        {row.name.trim().charAt(0) || "·"}
                        {row.activity !== "idle" && (
                          <span className="absolute bottom-0 right-0 flex size-4 items-center justify-center rounded-full border border-background bg-background">
                            <ActivityIcon activity={row.activity} />
                          </span>
                        )}
                      </span>
                    ) : (
                      <>
                        <span
                          className={cn(
                            "mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md border",
                            selected
                              ? "border-primary/25 bg-primary/10"
                              : "border-border/60 bg-muted/35",
                          )}
                        >
                          <ActivityIcon activity={row.activity} />
                        </span>
                        <span className="min-w-0 max-w-full flex-1 overflow-hidden">
                          <span className="block max-w-full truncate text-xs font-medium leading-5 text-foreground" title={row.name}>
                            {row.name}
                          </span>
                          <span className="flex min-w-0 max-w-full items-center gap-1.5 overflow-hidden text-[11px] leading-4">
                            <span
                              className={cn(
                                "min-w-0 truncate",
                                (row.activity === "running_read" ||
                                  row.activity === "running_write") &&
                                  "text-primary",
                                row.activity === "review" && "text-amber-400",
                                row.activity === "error" && "text-destructive",
                              )}
                            >
                              {label}
                            </span>
                            <span aria-hidden className="text-muted-foreground/40">
                              ·
                            </span>
                            <span className="shrink-0 text-muted-foreground/80">
                              {PROVIDER_LABELS[row.provider]}
                            </span>
                            {row.activity === "idle" && lastConversation && (
                              <span className="min-w-0 truncate text-muted-foreground/75">
                                · {lastConversation}
                              </span>
                            )}
                          </span>
                        </span>
                      </>
                    )}
                  </button>


                  {!collapsed && (
                    <>
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        className="absolute right-10 top-[11px] size-9 opacity-0 focus-visible:opacity-100 group-hover:opacity-100"
                        aria-label={`${row.name} 이름 변경`}
                        onClick={() => handleRename(row)}
                      >
                        <Pencil className="size-4" aria-hidden="true" />
                      </Button>
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        className="absolute right-1 top-[11px] size-9 opacity-0 focus-visible:opacity-100 group-hover:opacity-100"
                        aria-label={`${row.name} 삭제`}
                        disabled={
                          row.activity === "running_read" ||
                          row.activity === "running_write" ||
                          row.activity === "review"
                        }
                        onClick={() => handleDelete(row)}
                      >
                        <Trash2 className="size-4" aria-hidden="true" />
                      </Button>
                    </>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </nav>

      {!collapsed && (
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="세션 사이드바 너비 조절"
          aria-valuemin={MIN_WIDTH}
          aria-valuemax={MAX_WIDTH}
          aria-valuenow={Math.round(width)}
          tabIndex={0}
          onDoubleClick={() => setWidth(DEFAULT_WIDTH)}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft") {
              event.preventDefault();
              setWidth((current) => Math.max(MIN_WIDTH, current - 16));
            } else if (event.key === "ArrowRight") {
              event.preventDefault();
              setWidth((current) => Math.min(MAX_WIDTH, current + 16));
            } else if (event.key === "Home") {
              event.preventDefault();
              setWidth(MIN_WIDTH);
            } else if (event.key === "End") {
              event.preventDefault();
              setWidth(MAX_WIDTH);
            }
          }}
          onPointerDown={(event) => {
            dragRef.current = { startX: event.clientX, startWidth: width };
            setDragging(true);
            event.currentTarget.setPointerCapture(event.pointerId);
            event.preventDefault();
          }}
          onPointerMove={(event) => {
            const drag = dragRef.current;
            if (!drag) return;
            setWidth(
              Math.min(
                MAX_WIDTH,
                Math.max(MIN_WIDTH, drag.startWidth + event.clientX - drag.startX),
              ),
            );
          }}
          onPointerUp={(event) => {
            if (!dragRef.current) return;
            dragRef.current = null;
            setDragging(false);
            event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onPointerCancel={() => {
            dragRef.current = null;
            setDragging(false);
          }}
          className="group/splitter absolute inset-y-0 right-0 z-30 w-2 translate-x-1/2 cursor-col-resize touch-none outline-none"
        >
          <span className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border transition-colors group-hover/splitter:bg-primary/70 group-focus-visible/splitter:w-0.5 group-focus-visible/splitter:bg-primary group-active/splitter:w-0.5 group-active/splitter:bg-primary" />
        </div>
      )}
    </aside>
  );
}
