/**
 * Clickable workspace paths in chat answers. The agent names the files it
 * worked on as inline code (`.eud-agent/workspace/specs/opening.md`); inside a
 * {@link WorkspacePathProvider} an inline code span that resolves to a listed
 * workspace file renders as a link that opens the file's center document tab.
 * Every other inline code span (and every span outside a provider) renders as
 * Streamdown's ordinary inline code.
 */
import { createContext, useContext, useMemo, type ComponentProps, type ReactNode } from "react";
import type { Nodes, Root } from "mdast";
import type { Plugin } from "unified";
import type { WorkspaceFileEntry } from "@/lib/ipc";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { resolveChatFileLink, resolveMentionedWorkspacePath } from "@/components/workspaceLinks";

interface WorkspacePathContextValue {
  files: WorkspaceFileEntry[];
  readonly projectRoot?: string | null;
  onOpen: (file: WorkspaceFileEntry) => void;
}

const WorkspacePathContext = createContext<WorkspacePathContextValue | null>(null);

export function WorkspacePathProvider({
  files,
  projectRoot,
  onOpen,
  children,
}: WorkspacePathContextValue & { children: ReactNode }) {
  return (
    <WorkspacePathContext.Provider value={{ files, projectRoot, onOpen }}>
      {children}
    </WorkspacePathContext.Provider>
  );
}

const CHAT_FILE_LINK_PREFIX = "https://chat-file.invalid/";

/** Rewrite parsed links before Streamdown sanitizes local URL schemes. */
export function useWorkspaceLinkPlugin(): Plugin<[], Root> | undefined {
  const context = useContext(WorkspacePathContext);
  return useMemo(() => {
    if (!context) return undefined;
    return () => (tree: Root) => {
      const visit = (node: Nodes): void => {
        if (
          (node.type === "link" || node.type === "definition") &&
          !node.url.startsWith("#") &&
          !node.url.startsWith("//") &&
          (/^(?:file:|[a-z]:[\\/])/i.test(node.url) || !/^[a-z][a-z0-9+.-]*:/i.test(node.url))
        ) {
          node.url = CHAT_FILE_LINK_PREFIX + encodeURIComponent(node.url);
        }
        if ("children" in node) node.children.forEach(visit);
      };
      visit(tree);
    };
  }, [context]);
}

export function WorkspacePathLink({
  href,
  children,
  className,
  node: _node,
  ...props
}: ComponentProps<"a"> & { node?: unknown }) {
  const context = useContext(WorkspacePathContext);
  const local = href?.startsWith(CHAT_FILE_LINK_PREFIX) ?? false;
  let file: WorkspaceFileEntry | null = null;
  if (local && context && href) {
    try {
      file = resolveChatFileLink(
        context.files,
        decodeURIComponent(href.slice(CHAT_FILE_LINK_PREFIX.length)),
        context.projectRoot,
      );
    } catch (error) {
      if (!(error instanceof URIError)) throw error;
      file = null;
    }
  }
  if (local && !file) return <span>{children}</span>;
  return (
    <a
      {...props}
      href={href}
      className={cn("wrap-anywhere font-medium text-primary underline", className)}
      data-streamdown="link"
      target={local ? undefined : "_blank"}
      rel={local ? undefined : "noreferrer"}
      onClick={local ? (event) => {
        event.preventDefault();
        event.stopPropagation();
        if (file && context) context.onOpen(file);
      } : props.onClick}
      onAuxClick={local ? (event) => {
        event.preventDefault();
        event.stopPropagation();
      } : props.onAuxClick}
    >
      {children}
    </a>
  );
}

const INLINE_CODE_CLASS = "rounded bg-muted px-1.5 py-0.5 font-mono text-sm";

export function WorkspacePathCode({
  children,
  className,
  node: _node,
  ...props
}: ComponentProps<"code"> & { node?: unknown }) {
  const context = useContext(WorkspacePathContext);
  const file =
    context && typeof children === "string"
      ? resolveMentionedWorkspacePath(context.files, children)
      : null;
  if (!context || !file) {
    return (
      <code
        className={cn(INLINE_CODE_CLASS, className)}
        data-streamdown="inline-code"
        {...props}
      >
        {children}
      </code>
    );
  }
  return (
    <Button
      type="button"
      variant="link"
      className="inline h-auto p-0 align-baseline font-normal whitespace-normal break-all underline decoration-dotted underline-offset-2 hover:decoration-solid"
      aria-label={`${file.path} 파일 열기`}
      title={`${file.path} 열기`}
      onClick={() => context.onOpen(file)}
    >
      <code
        className={cn(INLINE_CODE_CLASS, "text-primary", className)}
        data-streamdown="inline-code"
        {...props}
      >
        {children}
      </code>
    </Button>
  );
}

/** Streamdown `components` for chat answers; module-level so it stays stable. */
export const CHAT_MARKDOWN_COMPONENTS = { inlineCode: WorkspacePathCode, a: WorkspacePathLink };
