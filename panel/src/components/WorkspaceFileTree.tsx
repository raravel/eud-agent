/**
 * Workspace file tree — the "파일" tab body of the project sidebar.
 *
 * A generic IDE-style explorer: one root node for the project root (the
 * directory holding the `.eap`), recursive folders/files below it, folders
 * first, chevron + indent per depth. No layout-specific grouping: whatever
 * relative paths the backend lists render as-is, so the tree keeps working
 * when document directories move under the project root.
 *
 * Selecting a file opens it as a document tab in the center column
 * (Orca-style split: tree right, document center). Search is the unified
 * backend filename+content search, debounced; while active the tree is
 * replaced by a flat result list. Collapsed folder state persists per
 * workspace id.
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
  RefreshCw,
  Search,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import type {
  WorkspaceFileEntry,
  WorkspaceListResponse,
} from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface WorkspaceFileTreeProps {
  workspace: WorkspaceListResponse;
  /** Path of the document tab active in the center column (highlighted). */
  selectedPath: string | null;
  loading: boolean;
  onSelect(file: WorkspaceFileEntry): void;
  onSearch(query: string): Promise<string[]>;
  onRefresh(): void;
}

interface TreeNode {
  name: string;
  /** Relative path from the project root ("" for the root node). */
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
function buildTree(files: WorkspaceFileEntry[], project: string): TreeNode {
  const root: TreeNode = { name: project, path: "", kind: "dir", children: [] };
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

/** Depth-first visible rows; folders open only when explicitly expanded. */
function visibleRows(root: TreeNode, expanded: Set<string>): TreeRow[] {
  const rows: TreeRow[] = [];
  const walk = (node: TreeNode, depth: number) => {
    rows.push({ node, depth });
    if (node.kind === "dir" && expanded.has(node.path)) {
      for (const child of node.children) walk(child, depth + 1);
    }
  };
  walk(root, 0);
  return rows;
}

/** Ancestor directory paths of a file path, root first (root files: root only). */
function ancestorDirectories(path: string): string[] {
  const segments = path.split("/");
  const ancestors = [""];
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

/** Folders start closed; only the project root is open until the user expands. */
function defaultExpandedDirectories(): Set<string> {
  return new Set([""]);
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
  const [searchPaths, setSearchPaths] = useState<string[] | null>(null);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const normalizedSearchQuery = searchQuery.trim();
  const searching = normalizedSearchQuery.length > 0;
  const tree = useMemo(
    () => buildTree(workspace.files, workspace.project),
    [workspace.files, workspace.project],
  );
  const rows = useMemo(
    () => visibleRows(tree, expandedDirectories),
    [tree, expandedDirectories],
  );
  const searchResults = useMemo(() => {
    if (searchPaths === null) return null;
    const byPath = new Map(workspace.files.map((file) => [file.path, file]));
    return searchPaths
      .map((path) => byPath.get(path))
      .filter((file): file is WorkspaceFileEntry => file !== undefined);
  }, [searchPaths, workspace.files]);

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
              aria-label="파일명 또는 내용 검색"
              aria-invalid={searchError ? true : undefined}
              value={searchQuery}
              placeholder="파일명 또는 내용 검색"
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
        </div>
        {searchError && (
          <p role="alert" className="px-1 pt-1.5 text-[11px] text-destructive">
            {searchError}
          </p>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {searching ? (
          searchResults === null ? (
            <div className="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
              <Spinner className="size-3.5" /> 검색 중…
            </div>
          ) : searchResults.length === 0 ? (
            <div className="px-3 py-2 text-xs text-muted-foreground">
              검색 결과가 없습니다.
            </div>
          ) : (
            <ul aria-label="검색 결과">
              {searchResults.map((file) => {
                const Icon = fileIcon(file.path);
                return (
                  <li key={file.path}>
                    <button
                      type="button"
                      title={file.path}
                      className="flex h-7 w-full items-center gap-1.5 px-2 text-left text-xs text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                      onClick={() => onSelect(file)}
                    >
                      <Icon className="size-3.5 shrink-0" aria-hidden="true" />
                      <span className="min-w-0 flex-1 truncate font-mono text-[11px]">
                        {file.path}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )
        ) : (
          <ul aria-label="프로젝트 파일 트리">
            {rows.map(({ node, depth }) => {
              const padding = ROOT_DEPTH_PADDING + depth * DEPTH_PADDING;
              if (node.kind === "dir") {
                const expanded = expandedDirectories.has(node.path);
                const Icon = expanded ? FolderOpen : Folder;
                return (
                  <li key={node.path || "."}>
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
