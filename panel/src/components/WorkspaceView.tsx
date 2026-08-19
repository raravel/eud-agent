import { type MouseEvent, useMemo } from "react";
import {
  BookOpen,
  Code2,
  FileText,
  FolderTree,
  RefreshCw,
  X,
} from "lucide-react";

import { Response } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
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
  onRefresh(): void;
  onClose(): void;
}

interface FileGroup {
  directory: string;
  files: WorkspaceFileEntry[];
}

const DIRECTORY_ORDER = ["specs", "plans", "decisions", "worklog", "source"];
const WORKSPACE_LINK_PREFIX = "https://workspace.invalid/";

function directoryRank(directory: string): number {
  const root = directory.split("/", 1)[0];
  const rank = DIRECTORY_ORDER.indexOf(root);
  return rank === -1 ? DIRECTORY_ORDER.length : rank;
}

function groupFiles(files: WorkspaceFileEntry[]): FileGroup[] {
  const groups = new Map<string, WorkspaceFileEntry[]>();
  for (const file of files) {
    const directory = file.path.includes("/")
      ? file.path.slice(0, file.path.lastIndexOf("/"))
      : "프로젝트 루트";
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

function fileName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
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
  onClose,
}: WorkspaceViewProps) {
  const groups = useMemo(() => groupFiles(workspace.files), [workspace.files]);
  const selected = workspace.files.find((file) => file.path === selectedPath) ?? null;
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
        </div>
      </header>

      <div className={cn("flex min-h-0 flex-1", embedded && "flex-col")}>
        <nav
          aria-label="워크스페이스 파일"
          className={cn(
            "shrink-0 overflow-y-auto bg-card/30 p-2",
            embedded
              ? "max-h-48 w-full border-b border-border"
              : "w-[17rem] border-r border-border",
          )}
        >
          {groups.length === 0 ? (
            <p className="p-3 text-xs text-muted-foreground">표시할 파일이 없습니다.</p>
          ) : (
            groups.map((group) => (
              <div key={group.directory} className="mb-3 last:mb-0">
                <div className="mb-1 flex items-center gap-1.5 px-2 text-[11px] font-medium text-muted-foreground">
                  <FolderTree className="size-3" aria-hidden="true" />
                  <span className="truncate">{group.directory}</span>
                </div>
                <div className="grid gap-0.5">
                  {group.files.map((file) => {
                    const active = file.path === selectedPath;
                    const wikiHome = file.path === "specs/index.md";
                    return (
                      <button
                        key={file.path}
                        type="button"
                        aria-current={active ? "page" : undefined}
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
                        <span className="min-w-0 flex-1 truncate">{fileName(file.path)}</span>
                        <span className="shrink-0 text-[10px] tabular-nums text-muted-foreground">
                          {formatBytes(file.size)}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            ))
          )}
        </nav>

        <article className="flex min-w-0 flex-1 flex-col overflow-hidden">
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
                왼쪽에서 문서를 선택하세요.
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
