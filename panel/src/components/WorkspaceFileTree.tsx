/**
 * Project file tree — the "파일" tab body of the project sidebar.
 *
 * A generic IDE-style explorer over the whole project root (the directory
 * holding the `.eap`): the root's own entries (`src/`, `dat/`, `maps/`,
 * `build/`, `.eud-agent/`, `project.eap`, ...) are the top level, with no row
 * for the root folder itself; recursive folders/files below them, folders
 * first, chevron + indent per depth. No layout-specific grouping: whatever project-relative paths the
 * backend lists render as-is; accepted/approved agent documents under
 * `.eud-agent/workspace` keep their state badges.
 *
 * Selecting a file opens it as a document tab in the center column
 * (Orca-style split: tree right, document center). Search is the backend
 * content search, debounced; while active the tree is replaced by matching
 * lines grouped per file, like an editor's search view, and a match opens
 * its file at that line (file names are Quick Open's, Ctrl+P). Collapsed
 * folder state persists per workspace id.
 */
import { useEffect, useMemo, useState } from "react";
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Code2,
  File as FileIcon,
  FileText,
  Folder,
  FolderOpen,
  MoreHorizontal,
  RefreshCw,
  Search,
  X,
} from "lucide-react";

import { toast } from "sonner";

import { HighlightedText } from "@/components/HighlightedText";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import {
  openProjectRootIn,
  type ProjectOpenTarget,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
  type WorkspaceSearchResponse,
} from "@/lib/ipc";
import type { FileLineTarget } from "@/lib/projectFiles";
import { cn } from "@/lib/utils";

export interface WorkspaceFileTreeProps {
  workspace: WorkspaceListResponse;
  /** Path of the document tab active in the center column (highlighted). */
  selectedPath: string | null;
  loading: boolean;
  /** `target` reveals a line, for a content search match. */
  onSelect(file: WorkspaceFileEntry, target?: FileLineTarget): void;
  onSearch(query: string): Promise<WorkspaceSearchResponse>;
  onRefresh(): void;
}

interface TreeNode {
  name: string;
  /** Relative path from the project root ("" for the unrendered root). */
  path: string;
  kind: "dir" | "file";
  children: TreeNode[];
  file?: WorkspaceFileEntry;
}

interface TreeRow {
  node: TreeNode;
  depth: number;
}

const EXPANDED_DIRECTORIES_KEY_PREFIX = "eud.workspace.expanded.";
const SEARCH_DEBOUNCE_MS = 200;
const ROOT_DEPTH_PADDING = 8;
const DEPTH_PADDING = 14;

const CODE_EXTENSIONS = [".eps", ".py", ".js", ".ts", ".jsx", ".tsx", ".json"];

function fileIcon(path: string) {
  if (path === "specs/index.md") return BookOpen;
  const lower = path.toLowerCase();
  if (lower.endsWith(".md")) return FileText;
  if (CODE_EXTENSIONS.some((extension) => lower.endsWith(extension))) {
    return Code2;
  }
  return FileIcon;
}

/** Build the folder/file trie from flat relative paths. */
function buildTree(files: WorkspaceFileEntry[]): TreeNode {
  const root: TreeNode = { name: "", path: "", kind: "dir", children: [] };
  const directories = new Map<string, TreeNode>([["", root]]);
  const ensureDirectory = (path: string): TreeNode => {
    const existing = directories.get(path);
    if (existing) return existing;
    const separator = path.lastIndexOf("/");
    const parent = ensureDirectory(separator === -1 ? "" : path.slice(0, separator));
    const node: TreeNode = {
      name: path.slice(separator + 1),
      path,
      kind: "dir",
      children: [],
    };
    parent.children.push(node);
    directories.set(path, node);
    return node;
  };
  for (const file of files) {
    const separator = file.path.lastIndexOf("/");
    const parent = ensureDirectory(separator === -1 ? "" : file.path.slice(0, separator));
    parent.children.push({
      name: file.path.slice(separator + 1),
      path: file.path,
      kind: "file",
      children: [],
      file,
    });
  }
  const sortTree = (node: TreeNode) => {
    node.children.sort(
      (left, right) =>
        Number(right.kind === "dir") - Number(left.kind === "dir") ||
        left.name.localeCompare(right.name),
    );
    for (const child of node.children) if (child.kind === "dir") sortTree(child);
  };
  sortTree(root);
  return root;
}

/**
 * Depth-first visible rows below the root, whose entries are the top level;
 * folders open only when explicitly expanded.
 */
function visibleRows(root: TreeNode, expanded: Set<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  const walk = (node: TreeNode, depth: number) => {
    rows.push({ node, depth });
    if (node.kind === "dir" && expanded.has(node.path)) {
      for (const child of node.children) walk(child, depth + 1);
    }
  };
  for (const child of root.children) walk(child, 0);
  return rows;
}

function directoryOf(path: string): string {
  const separator = path.lastIndexOf("/");
  return separator === -1 ? "" : path.slice(0, separator);
}

/** Ancestor directory paths of a file path, outermost first (root files: none). */
function ancestorDirectories(path: string): string[] {
  const segments = path.split("/");
  const ancestors: string[] = [];
  for (let index = 1; index < segments.length; index += 1) {
    ancestors.push(segments.slice(0, index).join("/"));
  }
  return ancestors;
}

/** Persisted expanded folders, or null when nothing was stored yet. */
function storedExpandedDirectories(workspaceId: string): Set<string> | null {
  if (typeof localStorage === "undefined") return null;
  try {
    const raw = localStorage.getItem(
      `${EXPANDED_DIRECTORIES_KEY_PREFIX}${workspaceId}`,
    );
    if (raw === null) return null;
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? new Set(parsed.filter((value): value is string => typeof value === "string"))
      : null;
  } catch {
    return null;
  }
}

/** Folders start closed until the user expands them. */
function defaultExpandedDirectories(): Set<string> {
  return new Set();
}

export function WorkspaceFileTree({
  workspace,
  selectedPath,
  loading,
  onSelect,
  onSearch,
  onRefresh,
}: WorkspaceFileTreeProps) {
  const [expandedDirectories, setExpandedDirectories] = useState(
    () =>
      storedExpandedDirectories(workspace.workspaceId) ??
      defaultExpandedDirectories(),
  );
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResult, setSearchResult] = useState<WorkspaceSearchResponse | null>(null);
  const [collapsedResults, setCollapsedResults] = useState<Set<string>>(() => new Set());
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const normalizedSearchQuery = searchQuery.trim();
  const searching = normalizedSearchQuery.length > 0;
  const tree = useMemo(
    () => buildTree(workspace.files),
    [workspace.files],
  );
  const rows = useMemo(
    () => visibleRows(tree, expandedDirectories),
    [tree, expandedDirectories],
  );
  const filesByPath = useMemo(
    () => new Map(workspace.files.map((file) => [file.path, file])),
    [workspace.files],
  );
  const searchMatchCount = useMemo(
    () =>
      searchResult?.files.reduce((total, file) => total + file.matches.length, 0) ?? 0,
    [searchResult],
  );

  useEffect(() => {
    try {
      localStorage.setItem(
        `${EXPANDED_DIRECTORIES_KEY_PREFIX}${workspace.workspaceId}`,
        JSON.stringify([...expandedDirectories].sort()),
      );
    } catch {
      // Persistence is optional; the current window keeps local state.
    }
  }, [expandedDirectories, workspace.workspaceId]);

  // Opening a document from anywhere (search, markdown link) reveals it in
  // the tree by expanding its ancestor folders.
  useEffect(() => {
    if (!selectedPath) return;
    const ancestors = ancestorDirectories(selectedPath);
    setExpandedDirectories((current) => {
      if (ancestors.every((path) => current.has(path))) return current;
      const next = new Set(current);
      for (const path of ancestors) next.add(path);
      return next;
    });
  }, [selectedPath]);

  useEffect(() => {
    setCollapsedResults(new Set());
    if (!normalizedSearchQuery) {
      setSearchResult(null);
      setSearchLoading(false);
      setSearchError(null);
      return;
    }

    // A newer query supersedes this one; its response is ignored.
    let cancelled = false;
    setSearchResult(null);
    setSearchLoading(true);
    setSearchError(null);
    const timeout = window.setTimeout(() => {
      void onSearch(normalizedSearchQuery)
        .then((result) => {
          if (!cancelled) setSearchResult(result);
        })
        .catch((searchFailure: unknown) => {
          if (!cancelled) {
            setSearchResult({
              workspaceId: workspace.workspaceId,
              query: normalizedSearchQuery,
              files: [],
              truncated: false,
            });
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
  }, [normalizedSearchQuery, onSearch, workspace.workspaceId]);

  const openRootIn = (target: ProjectOpenTarget) => {
    openProjectRootIn(target).catch((reason: unknown) => {
      toast.error(String(reason));
    });
  };

  const toggleSearchResult = (path: string) => {
    setCollapsedResults((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  const toggleDirectory = (path: string) => {
    setExpandedDirectories((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  return (
    <nav
      aria-label="워크스페이스 파일"
      className="flex h-full min-h-0 w-full flex-1 flex-col overflow-hidden"
    >
      <div className="shrink-0 border-b border-border bg-card/40 p-2">
        <div className="flex items-center gap-2">
          <div className="relative min-w-0 flex-1">
            <Search
              className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <Input
              type="search"
              aria-label="내용 검색"
              aria-invalid={searchError ? true : undefined}
              value={searchQuery}
              placeholder="내용 검색 (파일 이름은 Ctrl+P)"
              className="h-8 pl-8 pr-8 text-xs [&::-webkit-search-cancel-button]:appearance-none"
              onChange={(event) => setSearchQuery(event.target.value)}
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
                className="absolute right-1 top-1/2 flex size-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                onClick={() => setSearchQuery("")}
              >
                <X className="size-3.5" aria-hidden="true" />
              </button>
            ) : null}
          </div>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="size-8 shrink-0"
            aria-label="워크스페이스 새로 고침"
            title="새로 고침"
            disabled={loading}
            onClick={onRefresh}
          >
            {loading ? (
              <Spinner className="size-3.5" />
            ) : (
              <RefreshCw className="size-3.5" aria-hidden="true" />
            )}
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="size-8 shrink-0"
                aria-label="프로젝트 폴더 열기 메뉴"
                title="더 보기"
              >
                <MoreHorizontal className="size-3.5" aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => openRootIn("vscode")}>
                <Code2 aria-hidden="true" />
                VSCode로 열기
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => openRootIn("fileManager")}>
                <FolderOpen aria-hidden="true" />
                파일 탐색기로 열기
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        {searchError && (
          <p role="alert" className="px-1 pt-1.5 text-[11px] text-destructive">
            {searchError}
          </p>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {searching ? (
          searchResult === null ? (
            <div className="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
              <Spinner className="size-3.5" /> 검색 중…
            </div>
          ) : searchResult.files.length === 0 ? (
            <div className="px-3 py-2 text-xs text-muted-foreground">
              검색 결과가 없습니다.
            </div>
          ) : (
            <>
              <p role="status" className="px-3 pb-1 pt-1 text-[11px] text-muted-foreground">
                {searchResult.files.length}개 파일에서 {searchMatchCount}개 결과
                {searchResult.truncated && (
                  <span className="block text-amber-400">
                    결과가 많아 일부만 표시합니다. 검색어를 더 구체적으로 입력하세요.
                  </span>
                )}
              </p>
              <ul aria-label="검색 결과">
                {searchResult.files.map((result) => {
                  const Icon = fileIcon(result.path);
                  const collapsed = collapsedResults.has(result.path);
                  const name = result.path.slice(result.path.lastIndexOf("/") + 1);
                  // A file created after the last listing still opens by path.
                  const file = filesByPath.get(result.path) ?? { path: result.path, size: 0 };
                  return (
                    <li key={result.path}>
                      <button
                        type="button"
                        aria-expanded={!collapsed}
                        aria-label={`${result.path} 결과 ${result.matches.length}개 ${collapsed ? "펼치기" : "접기"}`}
                        title={result.path}
                        className="flex h-7 w-full items-center gap-1 px-2 text-left text-xs text-foreground/90 hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                        onClick={() => toggleSearchResult(result.path)}
                      >
                        {collapsed ? (
                          <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                        ) : (
                          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                        )}
                        <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                        <span className="shrink-0 truncate">{name}</span>
                        <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground/80">
                          {directoryOf(result.path)}
                        </span>
                        <span className="shrink-0 rounded-full bg-muted px-1.5 text-[10px] tabular-nums text-muted-foreground">
                          {result.matches.length}
                        </span>
                      </button>
                      {!collapsed && (
                        <ul aria-label={`${result.path} 일치 항목`}>
                          {result.matches.map((match) => (
                            <li key={match.line}>
                              <button
                                type="button"
                                title={`${result.path}:${match.line}`}
                                className="flex h-6 w-full items-center gap-2 pl-8 pr-2 text-left text-[11px] text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                                onClick={() => onSelect(file, { line: match.line })}
                              >
                                <span className="w-7 shrink-0 text-right tabular-nums text-muted-foreground/60">
                                  {match.line}
                                </span>
                                <HighlightedText
                                  text={match.preview}
                                  ranges={match.ranges}
                                  className="min-w-0 flex-1 truncate whitespace-pre font-mono"
                                />
                              </button>
                            </li>
                          ))}
                        </ul>
                      )}
                    </li>
                  );
                })}
              </ul>
            </>
          )
        ) : (
          <ul aria-label="프로젝트 파일 트리">
            {rows.map(({ node, depth }) => {
              const padding = ROOT_DEPTH_PADDING + depth * DEPTH_PADDING;
              if (node.kind === "dir") {
                const expanded = expandedDirectories.has(node.path);
                const Icon = expanded ? FolderOpen : Folder;
                return (
                  <li key={node.path}>
                    <button
                      type="button"
                      aria-expanded={expanded}
                      aria-label={`${node.name} 폴더 ${expanded ? "접기" : "펼치기"}`}
                      className="flex h-7 w-full items-center gap-1 pr-2 text-left text-xs text-foreground/90 hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                      style={{ paddingInlineStart: padding }}
                      onClick={() => toggleDirectory(node.path)}
                    >
                      {expanded ? (
                        <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                      ) : (
                        <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                      )}
                      <Icon className="size-3.5 shrink-0 text-amber-300/80" aria-hidden="true" />
                      <span className="min-w-0 flex-1 truncate">{node.name}</span>
                    </button>
                  </li>
                );
              }
              const active = node.path === selectedPath;
              const Icon = fileIcon(node.path);
              return (
                <li key={node.path}>
                  <button
                    type="button"
                    aria-current={active ? "page" : undefined}
                    title={node.path}
                    className={cn(
                      "flex h-7 w-full items-center gap-1.5 pr-2 text-left text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
                      active
                        ? "bg-primary/15 text-foreground"
                        : "text-muted-foreground hover:bg-muted hover:text-foreground",
                    )}
                    style={{ paddingInlineStart: padding + 14 }}
                    onClick={() => node.file && onSelect(node.file)}
                  >
                    <Icon
                      className={cn(
                        "size-3.5 shrink-0",
                        node.path === "specs/index.md"
                          ? "text-emerald-400"
                          : "text-muted-foreground",
                      )}
                      aria-hidden="true"
                    />
                    <span className="min-w-0 flex-1 truncate">{node.name}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </nav>
  );
}
