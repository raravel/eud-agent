/**
 * Parse the `read_file` / `file_write` tool payloads (EUD-068 tool_call args +
 * tool_result text) into a code-editor view: a path, the file's code/content,
 * and a Prism language. Only these two tools render as a CodeBlock (the user
 * asked for real code editing instead of raw JSON); every other tool keeps the
 * JSON request/result rows.
 *
 * The core ships args/result as compact JSON strings truncated to 4000 chars
 * (codex_client `TOOL_DATA_MAX_CHARS`), appending `…(잘림)` when cut. So parsing
 * is tolerant: a strict `JSON.parse` covers whole payloads; a lenient single-
 * field scan recovers the (possibly truncated) code/content when the JSON itself
 * is cut mid-string. Returns null when no code can be extracted (e.g. a failed
 * read whose result is an error message) — the caller then keeps the raw view.
 */
import type { AgentTool } from "@/components/AgentStream";

/** The marker the core appends to a tool arg/result string when it truncates. */
const TRUNCATION_MARK = "…(잘림)";

/** A parsed file-tool payload ready for the CodeBlock view. */
export interface FileToolView {
  /** "read" (read_file) or "write" (file_write). */
  mode: "read" | "write";
  /** Editor-relative file path (from the tool args). */
  path: string;
  /** The file content (read) or the code being written (write). */
  code: string;
  /** Prism language id derived from the path extension. */
  language: string;
  /** True when the core truncated the payload at 4000 chars. */
  truncated: boolean;
}

/**
 * Pull a single string field out of a (possibly truncated) compact-JSON string.
 * Strict parse first; on failure scan for `"key":"` and read the JSON string up
 * to its unescaped closing quote (or the end, when truncated).
 */
export function decodeJsonField(
  raw: string | undefined,
  key: string,
): string | null {
  if (!raw) return null;
  const cut = raw.indexOf(TRUNCATION_MARK);
  const text = cut === -1 ? raw : raw.slice(0, cut).trimEnd();

  try {
    const obj = JSON.parse(text) as unknown;
    if (obj && typeof obj === "object" && key in (obj as Record<string, unknown>)) {
      const value = (obj as Record<string, unknown>)[key];
      if (typeof value === "string") return value;
      if (value !== undefined) return String(value);
    }
  } catch {
    // Truncated or non-JSON — fall through to the lenient scan.
  }

  const marker = `"${key}"`;
  const at = text.indexOf(marker);
  if (at === -1) return null;
  // Advance past the key, the colon, and the opening quote.
  let i = at + marker.length;
  while (i < text.length && text[i] !== '"') {
    if (text[i] !== " " && text[i] !== ":" && text[i] !== "\t") return null;
    i += 1;
  }
  if (text[i] !== '"') return null;
  i += 1;
  let captured = "";
  for (; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === "\\") {
      captured += ch + (text[i + 1] ?? "");
      i += 1;
      continue;
    }
    if (ch === '"') break;
    captured += ch;
  }
  try {
    return JSON.parse(`"${captured}"`) as string;
  } catch {
    // Truncated mid-escape — unescape the common cases by hand.
    return captured
      .replace(/\\n/g, "\n")
      .replace(/\\t/g, "\t")
      .replace(/\\r/g, "\r")
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, "\\");
  }
}

/** Prism language id for a file path (defaults to plain text). */
export function languageForPath(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "py":
      return "python";
    case "eps":
      // epScript is C/JS-like (functions, `//` comments, braces, strings);
      // JavaScript highlighting reads cleanly for it.
      return "javascript";
    case "json":
      return "json";
    case "lua":
      return "lua";
    default:
      return "text";
  }
}

/**
 * Parse a tool into a {@link FileToolView}, or null when it is not a file tool
 * or no code could be extracted. `read_file` shows its RESULT content; a
 * `file_write` shows the code from its ARGS (the result is just an ok ack).
 */
export function parseFileTool(tool: AgentTool): FileToolView | null {
  const mode =
    tool.name === "read_file"
      ? "read"
      : tool.name === "file_write"
        ? "write"
        : null;
  if (!mode) return null;

  const path = decodeJsonField(tool.args, "path") ?? "";
  const source = mode === "write" ? tool.args : tool.detail;
  const code = decodeJsonField(source, mode === "write" ? "code" : "content");
  if (code === null) return null;

  return {
    mode,
    path,
    code,
    language: languageForPath(path),
    truncated: (source ?? "").includes(TRUNCATION_MARK),
  };
}
