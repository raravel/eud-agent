/**
 * Workspace document viewer — the center-column body of one open file tab.
 *
 * Markdown renders through the shared Response (Streamdown) surface with
 * workspace-relative links intercepted back into the file tree; other UTF-8
 * text renders as preformatted source. Loading/error/empty states are the
 * parent's (App) responsibility per selected path.
 */
import { useMemo, type MouseEvent } from "react";

import { Response } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import type { WorkspaceFileEntry, WorkspaceListResponse } from "@/lib/ipc";
import {
  WORKSPACE_LINK_PREFIX,
  resolveWorkspaceLink,
  rewriteWorkspaceMarkdownLinks,
} from "@/components/workspaceLinks";

export interface WorkspaceDocumentProps {
  workspace: WorkspaceListResponse;
  /** The file entry for this tab (null when its path left the workspace). */
  file: WorkspaceFileEntry | null;
  /** The tab's own content, independent of which tab is active. */
  content: string | null;
  loading: boolean;
  error: string | null;
  onSelect(file: WorkspaceFileEntry): void;
}

export function WorkspaceDocument({
  workspace,
  file,
  content,
  loading,
  error,
  onSelect,
}: WorkspaceDocumentProps) {
  const path = file?.path ?? null;
  const markdown = path?.toLowerCase().endsWith(".md") ?? false;
  const markdownContent = useMemo(
    () =>
      markdown && path && content !== null
        ? rewriteWorkspaceMarkdownLinks(content, path, workspace.files)
        : content,
    [markdown, content, path, workspace.files],
  );

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!(event.target instanceof Element) || !path) return;
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
      const linkedFile = workspace.files.find((candidate) => candidate.path === linkedPath);
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
    const linkedFile = resolveWorkspaceLink(workspace.files, path, href);
    if (linkedFile) onSelect(linkedFile);
  };

  const trustLabel =
    file?.state === "approved" && file.revision
      ? `승인된 계획 · r${file.revision}`
      : file?.state === "accepted" && file.revision
        ? `확정됨 · r${file.revision}`
        : "검토 대상 문서";

  return (
    <article aria-label="워크스페이스 문서" className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden">
      {file && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/30 px-4 py-2 text-xs">
          <span className="min-w-0 flex-1 truncate font-mono">{file.path}</span>
          <Badge variant="outline" className="shrink-0">
            {trustLabel}
          </Badge>
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {loading && content === null ? (
          <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
            <Spinner className="size-4" />
            파일을 여는 중…
          </div>
        ) : error ? (
          <div
            role="alert"
            className="rounded border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
          >
            {error}
          </div>
        ) : content === null ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            파일 트리에서 문서를 선택하세요.
          </div>
        ) : markdown ? (
          <div
            className="mx-auto max-w-4xl text-sm leading-7"
            onClick={handleClick}
          >
            <Response mode="static">{markdownContent ?? content}</Response>
          </div>
        ) : (
          <pre className="overflow-x-auto whitespace-pre-wrap break-words rounded border border-border bg-muted/30 p-4 font-mono text-xs leading-6">
            {content}
          </pre>
        )}
      </div>
    </article>
  );
}
