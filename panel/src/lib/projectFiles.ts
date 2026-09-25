import type { WorkspaceFileEntry, WorkspaceListResponse } from "@/lib/protocol";

/** One project-root file offered by the composer's `@` search. */
export interface ProjectFileSuggestion {
  workspaceId: string;
  path: string;
  size: number;
}

function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

/**
 * Rank project files for a `@` query: a file-name prefix beats a file-name
 * substring, which beats a path substring. `.eud-agent` harness files rank
 * after authoring files so `src/`, `maps/` and `dat/` come first.
 */
export function rankProjectFiles(
  workspaceId: string,
  files: readonly WorkspaceFileEntry[],
  query: string,
  limit: number,
): ProjectFileSuggestion[] {
  const needle = query.trim().toLowerCase();
  const scored: { entry: WorkspaceFileEntry; score: number }[] = [];
  for (const entry of files) {
    const path = entry.path.toLowerCase();
    const name = baseName(path);
    let score: number;
    if (needle.length === 0) score = 3;
    else if (name.startsWith(needle)) score = 0;
    else if (name.includes(needle)) score = 1;
    else if (path.includes(needle)) score = 2;
    else continue;
    if (path.startsWith(".eud-agent/")) score += 4;
    scored.push({ entry, score });
  }
  scored.sort(
    (left, right) =>
      left.score - right.score ||
      left.entry.path.length - right.entry.path.length ||
      left.entry.path.localeCompare(right.entry.path),
  );
  return scored.slice(0, limit).map(({ entry }) => ({
    workspaceId,
    path: entry.path,
    size: entry.size,
  }));
}

/** Wrap project file bytes as a browser File for the ordinary staging path. */
export function projectFileAsFile(path: string, bytes: Uint8Array): File {
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return new File([buffer], baseName(path));
}

const FILE_LIST_TTL_MS = 5000;
const FILE_SEARCH_LIMIT = 20;

export interface ProjectFileSource {
  search(query: string): Promise<ProjectFileSuggestion[]>;
  read(file: ProjectFileSuggestion): Promise<File>;
}

/**
 * The composers' `@` file source. One tree scan serves every keystroke of a
 * query; it is refreshed after a few seconds so new files appear without
 * reopening the project, and a failed scan is retried on the next keystroke.
 */
export function createProjectFileSource(
  list: () => Promise<WorkspaceListResponse>,
  readBytes: (workspaceId: string, path: string) => Promise<Uint8Array>,
): ProjectFileSource {
  let cached: { at: number; list: Promise<WorkspaceListResponse> } | null = null;
  return {
    async search(query) {
      if (cached === null || Date.now() - cached.at > FILE_LIST_TTL_MS) {
        const current = { at: Date.now(), list: list() };
        cached = current;
        current.list.catch(() => {
          if (cached === current) cached = null;
        });
      }
      const response = await cached.list;
      return rankProjectFiles(
        response.workspaceId,
        response.files,
        query,
        FILE_SEARCH_LIMIT,
      );
    },
    async read(file) {
      return projectFileAsFile(file.path, await readBytes(file.workspaceId, file.path));
    },
  };
}
