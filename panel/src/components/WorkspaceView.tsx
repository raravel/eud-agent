import { type MouseEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpen,
  ChevronDown,
  ChevronUp,
  ChevronRight,
  Code2,
  FileText,
  FolderTree,
  GripHorizontal,
  RefreshCw,
  Search,
  X,
} from "lucide-react";

import { Response } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Input } from "@/components/ui/input";
import type {
  WorkspaceFileEntry,
  WorkspaceListResponse,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface WorkspaceViewProps {
  workspace: WorkspaceListResponse;
  selectedPath: string | null;
  selectedContent: string | null;
  loading: boolean;
  error: string | null;
  embedded?: boolean;
  onSelect(file: WorkspaceFileEntry): void;
  onSearch(query: string): Promise<string[]>;
  onRefresh(): void;
  onClose(): void;
}

interface FileGroup {
  directory: string;
  files: WorkspaceFileEntry[];
}

const DIRECTORY_ORDER = ["specs", "plans", "decisions", "worklog", "source"];
const WORKSPACE_LINK_PREFIX = "https://workspace.invalid/";
const COLLAPSED_DIRECTORIES_KEY_PREFIX = "eud.workspace.collapsed.";
const SEARCH_DEBOUNCE_MS = 200;
const WORKSPACE_SPLIT_KEY = "eud.workspace.split";
const DEFAULT_TREE_HEIGHT = 192;
const MIN_PANEL_HEIGHT = 120;
const SPLITTER_HEIGHT = 48;
const COLLAPSE_THRESHOLD = 48;
const MAX_STORED_TREE_HEIGHT = 1600;

type WorkspaceCollapsedPanel = "tree" | "preview" | null;

interface WorkspaceSplitState {
  treeHeight: number;
  collapsed: WorkspaceCollapsedPanel;
}

function storedWorkspaceSplit(): WorkspaceSplitState {
  const fallback = { treeHeight: DEFAULT_TREE_HEIGHT, collapsed: null };
  if (typeof localStorage === "undefined") return fallback;
  try {
    const stored = localStorage.getItem(WORKSPACE_SPLIT_KEY);
    if (stored === null) return fallback;
    const parsed = JSON.parse(stored) as Partial<WorkspaceSplitState>;
    const treeHeight = Number(parsed.treeHeight);
    if (!Number.isFinite(treeHeight)) return fallback;
    const collapsed =
      parsed.collapsed === "tree" || parsed.collapsed === "preview"
        ? parsed.collapsed
        : null;
    return {
      treeHeight: Math.min(
        MAX_STORED_TREE_HEIGHT,
        Math.max(MIN_PANEL_HEIGHT, Math.round(treeHeight)),
      ),
      collapsed,
    };
  } catch {
    return fallback;
  }
}

function clampTreeHeight(height: number, containerHeight: number): number {
  const availableHeight = Math.max(0, containerHeight - SPLITTER_HEIGHT);
  const maximumHeight = Math.max(
    MIN_PANEL_HEIGHT,
    availableHeight - MIN_PANEL_HEIGHT,
  );
  return Math.round(
    Math.min(maximumHeight, Math.max(MIN_PANEL_HEIGHT, height)),
  );
}

function directoryRank(directory: string): number {
  const root = directory.split("/", 1)[0];
  const rank = DIRECTORY_ORDER.indexOf(root);
  return rank === -1 ? DIRECTORY_ORDER.length : rank;
}


function groupFiles(files: WorkspaceFileEntry[]): FileGroup[] {
  const groups = new Map<string, WorkspaceFileEntry[]>();
  for (const file of files) {
    const separator = file.path.indexOf("/");
    const directory =
      separator === -1 ? "프로젝트 루트" : file.path.slice(0, separator);
    const group = groups.get(directory) ?? [];
    group.push(file);
    groups.set(directory, group);
  }
  return [...groups.entries()]
    .sort(
      ([left], [right]) =>
        directoryRank(left) - directoryRank(right) || left.localeCompare(right),
    )
    .map(([directory, group]) => ({
      directory,
      files: group.sort(
        (left, right) =>
          Number(right.path === "specs/index.md") -
            Number(left.path === "specs/index.md") ||
          left.path.localeCompare(right.path),
      ),
    }));
}


function storedCollapsedDirectories(workspaceId: string): Set<string> {
  if (typeof localStorage === "undefined") return new Set();
  try {
    const raw = localStorage.getItem(
      `${COLLAPSED_DIRECTORIES_KEY_PREFIX}${workspaceId}`,
    );
    if (raw === null) return new Set();
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? new Set(parsed.filter((value): value is string => typeof value === "string"))
      : new Set();
  } catch {
    return new Set();
  }
}


function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KiB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}


function resolveWorkspaceLink(
  files: WorkspaceFileEntry[],
  currentPath: string,
  href: string,
): WorkspaceFileEntry | null {
  const rawPath = href.split(/[?#]/, 1)[0];
  let decodedPath: string;
  try {
    decodedPath = decodeURIComponent(rawPath);
  } catch {
    return null;
  }

  const segments = currentPath.includes("/")
    ? currentPath.slice(0, currentPath.lastIndexOf("/")).split("/")
    : [];
  for (const segment of decodedPath.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      if (segments.length === 0) return null;
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  if (segments.length === 0) return null;

  const resolved = segments.join("/");
  const leaf = segments.at(-1) ?? "";
  const candidates = [resolved];
  if (!leaf.includes(".")) {
    candidates.push(`${resolved}.md`, `${resolved}/index.md`);
  }
  return (
    candidates
      .map((candidate) => files.find((file) => file.path === candidate))
      .find((file): file is WorkspaceFileEntry => file !== undefined) ?? null
  );
}

function rewriteWorkspaceMarkdownLinks(
  markdown: string,
  currentPath: string,
  files: WorkspaceFileEntry[],
): string {
  return markdown.replace(
    /\]\(([^)\s]+)([^)]*)\)/g,
    (match, destination: string, suffix: string) => {
      if (
        destination.startsWith("#") ||
        destination.startsWith("/") ||
        destination.startsWith("//") ||
        destination.includes("\\") ||
        /^[a-z][a-z0-9+.-]*:/i.test(destination)
      ) {
        return match;
      }
      const linkedFile = resolveWorkspaceLink(files, currentPath, destination);
      return linkedFile
        ? `](${WORKSPACE_LINK_PREFIX}${encodeURIComponent(linkedFile.path)}${suffix})`
        : match;
    },
  );
}

export function WorkspaceView({
  workspace,
  selectedPath,
  selectedContent,
  loading,
  error,
  embedded = false,
  onSelect,
  onRefresh,
  onSearch,
  onClose,
}: WorkspaceViewProps) {
  const [collapsedDirectories, setCollapsedDirectories] = useState(() =>
    storedCollapsedDirectories(workspace.workspaceId),
  );
  const [searchCollapsedDirectories, setSearchCollapsedDirectories] = useState(
    () => new Set<string>(),
  );
  const [searchQuery, setSearchQuery] = useState("");
  const [searchPaths, setSearchPaths] = useState<string[] | null>(null);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [splitState, setSplitState] = useState(storedWorkspaceSplit);
  const splitContainerRef = useRef<HTMLDivElement>(null);
  const splitDragRef = useRef<{
    startY: number;
    startHeight: number;
    containerHeight: number;
  } | null>(null);
  const normalizedSearchQuery = searchQuery.trim();
  const visibleFiles = useMemo(() => {
    if (searchPaths === null) return workspace.files;
    const paths = new Set(searchPaths);
    return workspace.files.filter((file) => paths.has(file.path));
  }, [searchPaths, workspace.files]);
  const groups = useMemo(() => groupFiles(visibleFiles), [visibleFiles]);
  const activeCollapsedDirectories = normalizedSearchQuery
    ? searchCollapsedDirectories
    : collapsedDirectories;
  const selected = workspace.files.find((file) => file.path === selectedPath) ?? null;
  useEffect(() => {
    try {
      localStorage.setItem(
        `${COLLAPSED_DIRECTORIES_KEY_PREFIX}${workspace.workspaceId}`,
        JSON.stringify([...collapsedDirectories].sort()),
      );
    } catch {
      // Persistence is optional; the current window keeps local state.
    }
  }, [collapsedDirectories, workspace.workspaceId]);
  useEffect(() => {
    try {
      localStorage.setItem(WORKSPACE_SPLIT_KEY, JSON.stringify(splitState));
    } catch {
      // Split persistence is optional; the current window keeps local state.
    }
  }, [splitState]);
  useEffect(() => {
    if (!normalizedSearchQuery) {
      setSearchPaths(null);
      setSearchLoading(false);
      setSearchError(null);
      return;
    }

    let cancelled = false;
    setSearchPaths([]);
    setSearchLoading(true);
    setSearchError(null);
    const timeout = window.setTimeout(() => {
      void onSearch(normalizedSearchQuery)
        .then((paths) => {
          if (!cancelled) setSearchPaths(paths);
        })
        .catch((searchFailure: unknown) => {
          if (!cancelled) {
            setSearchPaths([]);
            setSearchError(`검색하지 못했습니다: ${String(searchFailure)}`);
          }
        })
        .finally(() => {
          if (!cancelled) setSearchLoading(false);
        });
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [normalizedSearchQuery, onSearch]);
  const toggleDirectory = (directory: string) => {
    const update = (current: Set<string>) => {
      const next = new Set(current);
      if (next.has(directory)) next.delete(directory);
      else next.add(directory);
      return next;
    };
    if (normalizedSearchQuery) setSearchCollapsedDirectories(update);
    else setCollapsedDirectories(update);
  };
  const updateSearchQuery = (query: string) => {
    setSearchQuery(query);
    setSearchCollapsedDirectories(new Set());
  };
  const markdown = selectedPath?.toLowerCase().endsWith(".md") ?? false;
  const markdownContent = useMemo(
    () =>
      markdown && selectedPath && selectedContent !== null
        ? rewriteWorkspaceMarkdownLinks(selectedContent, selectedPath, workspace.files)
        : selectedContent,
    [markdown, selectedContent, selectedPath, workspace.files],
  );
  const handleMarkdownClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!(event.target instanceof Element) || !selectedPath) return;
    const anchor = event.target.closest("a[href]");
    const href = anchor?.getAttribute("href");
    if (href?.startsWith(WORKSPACE_LINK_PREFIX)) {
      event.preventDefault();
      let linkedPath: string;
      try {
        linkedPath = decodeURIComponent(href.slice(WORKSPACE_LINK_PREFIX.length));
      } catch {
        return;
      }
      const linkedFile = workspace.files.find((file) => file.path === linkedPath);
      if (linkedFile) onSelect(linkedFile);
      return;
    }
    if (
      !href ||
      href.startsWith("#") ||
      href.startsWith("/") ||
      href.startsWith("//") ||
      href.includes("\\") ||
      /^[a-z][a-z0-9+.-]*:/i.test(href)
    ) {
      return;
    }
    event.preventDefault();
    const linkedFile = resolveWorkspaceLink(workspace.files, selectedPath, href);
    if (linkedFile) onSelect(linkedFile);
  };
  const trustLabel = selected?.source
    ? "읽기 전용 소스"
    : selected?.state === "approved" && selected.revision
      ? `승인된 계획 · r${selected.revision}`
      : selected?.state === "accepted" && selected.revision
        ? `확정됨 · r${selected.revision}`
        : "검토 대상 문서";

  return (
    <section
      aria-label="프로젝트 워크스페이스"
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
      }}
      className={cn(
        "flex min-h-0 flex-col overflow-hidden bg-background",
        embedded ? "h-full flex-1" : "max-h-[68vh] min-h-[360px] border-t border-border",
      )}
    >
      <header className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <FolderTree className="size-4 text-emerald-400" aria-hidden="true" />
            <h2 className="text-sm font-semibold">프로젝트 워크스페이스</h2>
          </div>
          <p className="truncate text-xs text-muted-foreground">{workspace.project}</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="min-h-9 gap-1.5"
            disabled={loading}
            onClick={onRefresh}
          >
            {loading ? <Spinner className="size-3.5" /> : <RefreshCw className="size-3.5" />}
            새로 고침
          </Button>
          {!embedded && (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="size-10"
              aria-label="워크스페이스 닫기"
              onClick={onClose}
            >
              <X className="size-4" aria-hidden="true" />
            </Button>
          )}
        </div>
      </header>

      <div ref={splitContainerRef} className="flex min-h-0 flex-1 flex-col">
        <nav
          aria-label="워크스페이스 파일"
          hidden={splitState.collapsed === "tree"}
          style={
            splitState.collapsed === null
              ? {
                  height: splitState.treeHeight,
                  maxHeight: `calc(100% - ${MIN_PANEL_HEIGHT + SPLITTER_HEIGHT}px)`,
                }
              : undefined
          }
          className={cn(
            "w-full overflow-y-auto bg-card/30 p-2",
            splitState.collapsed === "preview"
              ? "min-h-0 flex-1"
              : "shrink-0",
            splitState.collapsed === "tree" && "hidden",
          )}
        >
          <div className="sticky top-0 z-10 bg-card/95 pb-2 backdrop-blur-sm">
            <div className="relative">
              <Search
                className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <Input
                type="search"
                aria-label="파일명 또는 내용 검색"
                aria-invalid={searchError ? true : undefined}
                value={searchQuery}
                placeholder="파일명 또는 내용 검색"
                className="h-9 pl-8 pr-8 text-xs [&::-webkit-search-cancel-button]:appearance-none"
                onChange={(event) => updateSearchQuery(event.target.value)}
              />
              {searchLoading ? (
                <Spinner
                  className="absolute right-2.5 top-1/2 size-3.5 -translate-y-1/2"
                  aria-hidden="true"
                />
              ) : searchQuery ? (
                <button
                  type="button"
                  aria-label="검색어 지우기"
                  className="absolute right-1 top-1/2 flex size-7 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  onClick={() => updateSearchQuery("")}
                >
                  <X className="size-3.5" aria-hidden="true" />
                </button>
              ) : null}
            </div>
            {searchError && (
              <p role="alert" className="px-1 pt-1.5 text-[11px] text-destructive">
                {searchError}
              </p>
            )}
          </div>
          {groups.length === 0 ? (
            <div
              className="flex items-center gap-2 p-3 text-xs text-muted-foreground"
              aria-live="polite"
            >
              {searchLoading && <Spinner className="size-3.5" />}
              {searchLoading
                ? "검색 중…"
                : normalizedSearchQuery
                  ? "검색 결과가 없습니다."
                  : "표시할 파일이 없습니다."}
            </div>
          ) : (
            groups.map((group) => {
              const collapsed = activeCollapsedDirectories.has(group.directory);
              return (
                <div key={group.directory} className="mb-2 last:mb-0">
                  <button
                    type="button"
                    aria-expanded={!collapsed}
                    aria-label={`${group.directory} 폴더 ${collapsed ? "펼치기" : "접기"}`}
                    className="mb-1 flex min-h-9 w-full items-center gap-1.5 rounded px-1.5 text-left text-[11px] font-medium text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={() => toggleDirectory(group.directory)}
                  >
                    {collapsed ? (
                      <ChevronRight className="size-3 shrink-0" aria-hidden="true" />
                    ) : (
                      <ChevronDown className="size-3 shrink-0" aria-hidden="true" />
                    )}
                    <FolderTree className="size-3 shrink-0" aria-hidden="true" />
                    <span className="min-w-0 flex-1 truncate">{group.directory}</span>
                    <span className="shrink-0 tabular-nums">{group.files.length}</span>
                  </button>
                  {!collapsed && (
                    <div className="grid gap-0.5">
                      {group.files.map((file) => {
                        const active = file.path === selectedPath;
                        const wikiHome = file.path === "specs/index.md";
                        return (
                          <button
                            key={file.path}
                            type="button"
                            aria-current={active ? "page" : undefined}
                            title={file.path}
                            className={cn(
                              "flex min-h-10 w-full items-center gap-2 rounded px-2 py-2 text-left text-xs transition-colors",
                              active
                                ? "bg-primary/15 text-foreground ring-1 ring-primary/30"
                                : "text-muted-foreground hover:bg-muted hover:text-foreground",
                            )}
                            onClick={() => onSelect(file)}
                          >
                            {wikiHome ? (
                              <BookOpen className="size-3.5 shrink-0 text-emerald-400" aria-hidden="true" />
                            ) : file.source ? (
                              <Code2 className="size-3.5 shrink-0 text-sky-400" aria-hidden="true" />
                            ) : (
                              <FileText className="size-3.5 shrink-0 text-emerald-400" aria-hidden="true" />
                            )}
                            <span className="min-w-0 flex-1 truncate">
                              {group.directory === "프로젝트 루트"
                                ? file.path
                                : file.path.slice(group.directory.length + 1)}
                            </span>
                            <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                              {formatBytes(file.size)}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })
          )}
        </nav>

        <div className="flex h-12 shrink-0 items-center border-y border-border bg-card/60">
          <div
            role="separator"
            aria-orientation="horizontal"
            aria-label="파일 트리와 문서 높이 조절"
            title="드래그하여 파일 트리와 문서 높이 조절"
            onDoubleClick={() =>
              setSplitState({
                treeHeight: DEFAULT_TREE_HEIGHT,
                collapsed: null,
              })
            }
            onPointerDown={(event) => {
              const bounds = splitContainerRef.current?.getBoundingClientRect();
              if (!bounds) return;
              splitDragRef.current = {
                startY: event.clientY,
                startHeight:
                  splitState.collapsed === "tree"
                    ? 0
                    : splitState.collapsed === "preview"
                      ? bounds.height - SPLITTER_HEIGHT
                      : splitState.treeHeight,
                containerHeight: bounds.height,
              };
              event.currentTarget.setPointerCapture(event.pointerId);
              event.preventDefault();
            }}
            onPointerMove={(event) => {
              const drag = splitDragRef.current;
              if (!drag) return;
              const nextHeight =
                drag.startHeight + event.clientY - drag.startY;
              const availableHeight =
                drag.containerHeight - SPLITTER_HEIGHT;
              if (nextHeight <= COLLAPSE_THRESHOLD) {
                setSplitState((current) => ({
                  ...current,
                  collapsed: "tree",
                }));
              } else if (
                nextHeight >= availableHeight - COLLAPSE_THRESHOLD
              ) {
                setSplitState((current) => ({
                  ...current,
                  collapsed: "preview",
                }));
              } else {
                setSplitState({
                  treeHeight: clampTreeHeight(
                    nextHeight,
                    drag.containerHeight,
                  ),
                  collapsed: null,
                });
              }
            }}
            onPointerUp={(event) => {
              if (!splitDragRef.current) return;
              splitDragRef.current = null;
              if (event.currentTarget.hasPointerCapture(event.pointerId)) {
                event.currentTarget.releasePointerCapture(event.pointerId);
              }
            }}
            onPointerCancel={() => {
              splitDragRef.current = null;
            }}
            className="group/splitter relative h-full min-w-0 flex-1 cursor-row-resize touch-none outline-none"
          >
            <span className="absolute inset-x-2 top-1/2 h-px -translate-y-1/2 bg-border transition-colors group-hover/splitter:h-0.5 group-hover/splitter:bg-primary/70 group-active/splitter:h-0.5 group-active/splitter:bg-primary" />
            <span className="pointer-events-none absolute left-1/2 top-1/2 flex h-5 w-8 -translate-x-1/2 -translate-y-1/2 items-center justify-center bg-card text-muted-foreground">
              <GripHorizontal className="size-4" aria-hidden="true" />
            </span>
          </div>
          {splitState.collapsed === null ? (
            <>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="size-12 shrink-0 rounded-none"
                aria-label="파일 트리 접기"
                title="파일 트리 접기"
                onClick={() =>
                  setSplitState((current) => ({
                    ...current,
                    collapsed: "tree",
                  }))
                }
              >
                <ChevronUp className="size-3.5" aria-hidden="true" />
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="size-12 shrink-0 rounded-none"
                aria-label="문서 미리보기 접기"
                title="문서 미리보기 접기"
                onClick={() =>
                  setSplitState((current) => ({
                    ...current,
                    collapsed: "preview",
                  }))
                }
              >
                <ChevronDown className="size-3.5" aria-hidden="true" />
              </Button>
            </>
          ) : splitState.collapsed === "tree" ? (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="size-12 shrink-0 rounded-none"
              aria-label="파일 트리 펼치기"
              title="파일 트리 펼치기"
              onClick={() =>
                setSplitState((current) => ({
                  ...current,
                  collapsed: null,
                }))
              }
            >
              <ChevronDown className="size-3.5" aria-hidden="true" />
            </Button>
          ) : (
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="size-12 shrink-0 rounded-none"
              aria-label="문서 미리보기 펼치기"
              title="문서 미리보기 펼치기"
              onClick={() =>
                setSplitState((current) => ({
                  ...current,
                  collapsed: null,
                }))
              }
            >
              <ChevronUp className="size-3.5" aria-hidden="true" />
            </Button>
          )}
        </div>

        <article
          hidden={splitState.collapsed === "preview"}
          className={cn(
            "flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden",
            splitState.collapsed === "preview" && "hidden",
          )}
        >
          {selected && (
            <div className="flex items-center gap-2 border-b border-border bg-muted/30 px-4 py-2 text-xs">
              <span className="min-w-0 flex-1 truncate font-mono">{selected.path}</span>
              <Badge variant="outline" className="shrink-0">
                {trustLabel}
              </Badge>
            </div>
          )}
          <div className="min-h-0 flex-1 overflow-y-auto p-4">
            {loading && selectedContent === null ? (
              <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
                <Spinner className="size-4" />
                파일을 여는 중…
              </div>
            ) : error ? (
              <div role="alert" className="rounded border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive">
                {error}
              </div>
            ) : selectedContent === null ? (
              <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
                파일 트리에서 문서를 선택하세요.
              </div>
            ) : markdown ? (
              <div
                className="mx-auto max-w-4xl text-sm leading-7"
                onClick={handleMarkdownClick}
              >
                <Response mode="static">{markdownContent ?? selectedContent}</Response>
              </div>
            ) : (
              <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded border border-border bg-muted/30 p-4 font-mono text-xs leading-6">
                {selectedContent}
              </pre>
            )}
          </div>
        </article>
      </div>
    </section>
  );
}
