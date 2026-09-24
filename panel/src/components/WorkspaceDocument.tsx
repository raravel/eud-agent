/**
 * Project document viewer — the center-column body of one open file tab.
 *
 * Markdown renders through the shared Response (Streamdown) surface with
 * relative links intercepted back into the file tree; EPS source opens in a
 * read-only Monaco surface highlighted with the TypeScript grammar (EPS is
 * C-like enough for it; the language service is muted in `@/editor/monaco`);
 * other UTF-8 text (Python, JSON, ...) renders as preformatted source. A
 * binary or oversized file shows the backend's notice instead of content.
 * Loading/error/empty states are the parent's (App) responsibility per
 * selected path; the tab path drives rendering even when the file has left
 * the listed tree.
 */
import { Suspense, lazy, useMemo, type MouseEvent } from "react";

import { Response } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import {
  WORKSPACE_DOCUMENT_PREFIX,
  type WorkspaceFileEntry,
  type WorkspaceListResponse,
} from "@/lib/ipc";
import {
  WORKSPACE_LINK_PREFIX,
  resolveWorkspaceLink,
  rewriteWorkspaceMarkdownLinks,
} from "@/components/workspaceLinks";

export interface WorkspaceDocumentProps {
  workspace: WorkspaceListResponse;
  /** The tab's project-relative path. */
  path: string;
  /** The listed file entry for this tab (null for a path that left the tree). */
  file: WorkspaceFileEntry | null;
  /** The tab's own content, independent of which tab is active. */
  content: string | null;
  loading: boolean;
  error: string | null;
  /** Why a listed file stays closed (binary or oversized); shown in place of content. */
  notice: string | null;
  onSelect(file: WorkspaceFileEntry): void;
}

// Monaco stays in its own async chunk (see MonacoEditor.tsx); it loads the
// first time an EPS tab opens.
const MonacoEditor = lazy(() => import("@/components/MonacoEditor"));

/** Header badge: trusted document state, else what kind of file this is. */
function trustLabel(path: string, file: WorkspaceFileEntry | null): string {
  if (file?.state === "approved" && file.revision) {
    return `승인된 계획 · r${file.revision}`;
  }
  if (file?.state === "accepted" && file.revision) {
    return `확정됨 · r${file.revision}`;
  }
  if (path.startsWith(WORKSPACE_DOCUMENT_PREFIX)) return "검토 대상 문서";
  return "프로젝트 파일";
}

export function WorkspaceDocument({
  workspace,
  path,
  file,
  content,
  loading,
  error,
  notice,
  onSelect,
}: WorkspaceDocumentProps) {
  const lowerPath = path.toLowerCase();
  const markdown = lowerPath.endsWith(".md");
  const eps = lowerPath.endsWith(".eps");
  // The editor owns its scroll surface, so its wrapper drops the padded
  // document scroll container the other renderings use.
  const showEditor = eps && content !== null && !error && !notice;
  const markdownContent = useMemo(
    () =>
      markdown && content !== null
        ? rewriteWorkspaceMarkdownLinks(content, path, workspace.files)
        : content,
    [markdown, content, path, workspace.files],
  );

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!(event.target instanceof Element)) return;
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

  return (
    <article aria-label="프로젝트 문서" className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/30 px-4 py-2 text-xs">
        <span className="min-w-0 flex-1 truncate font-mono">{path}</span>
        <Badge variant="outline" className="shrink-0">
          {trustLabel(path, file)}
        </Badge>
      </div>
      <div
        className={
          showEditor
            ? "min-h-0 flex-1 overflow-hidden"
            : "min-h-0 flex-1 overflow-y-auto p-4"
        }
      >
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
        ) : notice ? (
          <div
            role="status"
            className="flex h-full items-center justify-center px-6 text-center text-sm text-muted-foreground"
          >
            {notice}
          </div>
        ) : content === null ? (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            파일 트리에서 문서를 선택하세요.
          </div>
        ) : showEditor ? (
          <Suspense
            fallback={
              <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
                <Spinner className="size-4" />
                편집기를 여는 중…
              </div>
            }
          >
            <MonacoEditor
              value={content}
              language="typescript"
              height="100%"
              readOnly
              ariaLabel={`${path} 소스`}
            />
          </Suspense>
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
