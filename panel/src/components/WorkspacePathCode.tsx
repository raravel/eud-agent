/**
 * Clickable workspace paths in chat answers. The agent names the files it
 * worked on as inline code (`.eud-agent/workspace/specs/opening.md`); inside a
 * {@link WorkspacePathProvider} an inline code span that resolves to a listed
 * workspace file renders as a link that opens the file's center document tab.
 * Every other inline code span (and every span outside a provider) renders as
 * Streamdown's ordinary inline code.
 */
import { createContext, useContext, type ComponentProps, type ReactNode } from "react";
import type { WorkspaceFileEntry } from "@/lib/ipc";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { resolveMentionedWorkspacePath } from "@/components/workspaceLinks";

interface WorkspacePathContextValue {
  files: WorkspaceFileEntry[];
  onOpen: (file: WorkspaceFileEntry) => void;
}

const WorkspacePathContext = createContext<WorkspacePathContextValue | null>(null);

export function WorkspacePathProvider({
  files,
  onOpen,
  children,
}: WorkspacePathContextValue & { children: ReactNode }) {
  return (
    <WorkspacePathContext.Provider value={{ files, onOpen }}>
      {children}
    </WorkspacePathContext.Provider>
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
export const CHAT_MARKDOWN_COMPONENTS = { inlineCode: WorkspacePathCode };
