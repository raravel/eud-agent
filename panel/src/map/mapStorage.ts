/**
 * Per-project storage of the Map window's own view state (layout, surface,
 * window geometry). Each project's Map window keeps its own copy under
 * `<base>/2:<projectKey>`; the single-project `<base>/1` value is copied into
 * a project's key the first time that project reads it and is kept as is.
 * Storage can be unavailable (private mode, blocked site data): every access
 * falls back to "nothing stored".
 */
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isForeignProjectPayload } from "../lib/ipc";

const MAP_WINDOW_PREFIX = "map-agent-";

/**
 * The project key of this Map window: its label's project suffix. The label
 * is `map-agent-<first 12 hex of the project id>`, so each project's window
 * has its own key; outside a Map window the key is `default`.
 */
export function currentMapProjectKey(): string {
  try {
    const label = getCurrentWindow().label;
    return label.startsWith(MAP_WINDOW_PREFIX)
      ? label.slice(MAP_WINDOW_PREFIX.length)
      : "default";
  } catch {
    return "default";
  }
}

export function projectStorageKey(base: string, projectKey: string): string {
  return `${base}/2:${projectKey}`;
}

/** The stored value of `base` for `projectKey`, migrating `<base>/1` once. */
export function readProjectState(base: string, projectKey: string): string | null {
  try {
    const key = projectStorageKey(base, projectKey);
    const stored = localStorage.getItem(key);
    if (stored !== null) return stored;
    const legacy = localStorage.getItem(`${base}/1`);
    if (legacy !== null) localStorage.setItem(key, legacy);
    return legacy;
  } catch {
    return null;
  }
}

export function writeProjectState(base: string, projectKey: string, value: string): void {
  try {
    localStorage.setItem(projectStorageKey(base, projectKey), value);
  } catch {
    // A view preference that cannot be stored is simply not remembered.
  }
}

/** Whether an event names another project than `projectId` (when known). */
export function isOtherProjectEvent(payload: unknown, projectId: string | undefined): boolean {
  return isForeignProjectPayload(payload, projectId);
}
