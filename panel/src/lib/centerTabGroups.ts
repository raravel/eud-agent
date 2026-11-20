/**
 * Center-column tab groups, one per session: the documents, reference
 * articles and DAT wiki / 사운드 tabs a session opened, plus the tab it last
 * showed. Switching sessions swaps the whole group, and the groups persist per
 * project in localStorage so they come back after a restart.
 */

export interface CenterTabReference {
  /** The tab id (`reference:<url or hit id>`). */
  id: string;
  /** The search hit the article is fetched with. */
  hitId: string;
  title: string;
}

export interface CenterTabGroup {
  documents: string[];
  references: CenterTabReference[];
  datWiki: boolean;
  sounds: boolean;
  active: string;
}

export type CenterTabGroups = Record<string, CenterTabGroup>;

const STORAGE_PREFIX = "eud-agent.centerTabs/1:";

export function emptyCenterTabGroup(): CenterTabGroup {
  return { documents: [], references: [], datWiki: false, sounds: false, active: "chat" };
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function parseGroup(value: unknown): CenterTabGroup | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const references = Array.isArray(raw.references)
    ? raw.references.flatMap((item): CenterTabReference[] => {
        if (!item || typeof item !== "object") return [];
        const ref = item as Record<string, unknown>;
        return typeof ref.id === "string" && typeof ref.hitId === "string" && typeof ref.title === "string"
          ? [{ id: ref.id, hitId: ref.hitId, title: ref.title }]
          : [];
      })
    : [];
  return {
    documents: strings(raw.documents),
    references,
    datWiki: raw.datWiki === true,
    sounds: raw.sounds === true,
    active: typeof raw.active === "string" ? raw.active : "chat",
  };
}

export function parseCenterTabGroups(text: string | null): CenterTabGroups {
  if (!text) return {};
  try {
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const groups: CenterTabGroups = {};
    for (const [sessionId, raw] of Object.entries(value)) {
      const group = parseGroup(raw);
      if (group) groups[sessionId] = group;
    }
    return groups;
  } catch {
    return {};
  }
}

/** True when the group holds nothing worth remembering. */
export function isEmptyCenterTabGroup(group: CenterTabGroup): boolean {
  return (
    group.documents.length === 0 &&
    group.references.length === 0 &&
    !group.datWiki &&
    !group.sounds &&
    group.active === "chat"
  );
}

/**
 * Drops the groups of sessions that no longer exist and empty groups, so the
 * stored map never outgrows the project's sessions.
 */
export function pruneCenterTabGroups(
  groups: CenterTabGroups,
  liveSessionIds: ReadonlySet<string>,
): CenterTabGroups {
  const next: CenterTabGroups = {};
  for (const [sessionId, group] of Object.entries(groups)) {
    if (liveSessionIds.has(sessionId) && !isEmptyCenterTabGroup(group)) next[sessionId] = group;
  }
  return next;
}

export function loadCenterTabGroups(projectPath: string): CenterTabGroups {
  try {
    return parseCenterTabGroups(localStorage.getItem(`${STORAGE_PREFIX}${projectPath}`));
  } catch {
    return {};
  }
}

export function saveCenterTabGroups(projectPath: string, groups: CenterTabGroups): void {
  try {
    const key = `${STORAGE_PREFIX}${projectPath}`;
    if (Object.keys(groups).length === 0) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(groups));
  } catch {
    // Tab persistence is a convenience; the open tabs stay in this window.
  }
}

/**
 * The tab to show after a center tab key: Alt+1..8 picks that position,
 * Alt+9 the last tab, and Ctrl+PageUp/PageDown step with wrap-around.
 * Returns null for any other key.
 */
export function centerTabForKey(
  event: Pick<KeyboardEvent, "altKey" | "ctrlKey" | "metaKey" | "shiftKey" | "code" | "key">,
  tabIds: readonly string[],
  active: string,
): string | null {
  if (tabIds.length === 0 || event.metaKey || event.shiftKey) return null;
  if (event.altKey && !event.ctrlKey) {
    const match = /^Digit([1-9])$/.exec(event.code);
    if (!match) return null;
    const digit = Number(match[1]);
    if (digit === 9) return tabIds[tabIds.length - 1];
    return tabIds[digit - 1] ?? null;
  }
  if (event.ctrlKey && !event.altKey && (event.key === "PageUp" || event.key === "PageDown")) {
    const index = Math.max(0, tabIds.indexOf(active));
    const step = event.key === "PageUp" ? -1 : 1;
    return tabIds[(index + step + tabIds.length) % tabIds.length];
  }
  return null;
}
