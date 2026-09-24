import type { WorkspaceFileEntry } from "@/lib/ipc";

/**
 * Shared workspace document-link plumbing.
 *
 * Relative Markdown links inside workspace documents are rewritten to a
 * `https://workspace.invalid/<encoded path>` URL so the renderer treats them as
 * normal links; a click handler in the document view resolves them back to a
 * workspace file and routes navigation through `onSelect` (never the WebView).
 */
export const WORKSPACE_LINK_PREFIX = "https://workspace.invalid/";

/** Resolve a relative link target against the document's own path. */
export function resolveWorkspaceLink(
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

/** Project-relative prefix of the agent's workspace documents. */
const WORKSPACE_DOCUMENT_PREFIX = ".eud-agent/workspace/";

/**
 * Resolve a file path the agent wrote as text (typically inline code in a chat
 * answer) to a workspace file. Accepts `./` prefixes, `\` separators, and a
 * trailing `:line` or `:line:col`; a path relative to the agent workspace
 * (`specs/opening.md`) also matches its project-relative document. Anything
 * that is not exactly a listed file resolves to null.
 */
export function resolveMentionedWorkspacePath(
  files: WorkspaceFileEntry[],
  text: string,
): WorkspaceFileEntry | null {
  const path = text
    .trim()
    .replace(/\\/g, "/")
    .replace(/:\d+(?::\d+)?$/, "")
    .replace(/^(?:\.\/)+/, "");
  if (!path || path.startsWith("/") || /^[a-z]:/i.test(path)) return null;
  for (const candidate of [path, `${WORKSPACE_DOCUMENT_PREFIX}${path}`]) {
    const file = files.find((entry) => entry.path === candidate);
    if (file) return file;
  }
  return null;
}

/**
 * Rewrite every relative Markdown link that resolves to a workspace file into
 * the internal prefix form handled by {@link resolveWorkspaceLink} on click.
 */
export function rewriteWorkspaceMarkdownLinks(
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
