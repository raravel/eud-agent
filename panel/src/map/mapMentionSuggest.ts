import type { MapLocation, SavedSelection } from "./mapProtocol";

/** Rendered rows in the `@` listbox; the query narrows a longer catalog. */
export const MAP_MENTION_SUGGESTION_LIMIT = 24;

export type MapMentionSuggestion =
  | {
      key: string;
      kind: "region";
      label: string;
      detail: string;
      selection: SavedSelection;
    }
  | {
      key: string;
      kind: "location";
      label: string;
      detail: string;
      location: MapLocation;
    };

/**
 * `@` completions for the Map prompt: saved selections first (they are the
 * request-specific authority), then the candidate's locations. Labels match
 * the chips the palette and canvas create so the tray reads the same either way.
 */
export function mapMentionSuggestions(
  query: string,
  selections: readonly SavedSelection[],
  locations: readonly MapLocation[],
): MapMentionSuggestion[] {
  const normalized = query.trim().toLocaleLowerCase();
  const matches = (haystack: string) =>
    normalized.length === 0 || haystack.toLocaleLowerCase().includes(normalized);
  const suggestions: MapMentionSuggestion[] = [];
  for (const selection of selections) {
    const label = `${selection.role}:${selection.label}`;
    if (!matches(label)) continue;
    suggestions.push({
      key: `region:${selection.id}`,
      kind: "region",
      label,
      detail: `${selection.selectedCells}셀 · ${selection.layers.join(", ")}`,
      selection,
    });
  }
  for (const location of locations) {
    const label = `location:#${location.id} ${location.name || "이름 없음"}`;
    if (!matches(`#${location.id} ${location.name}`)) continue;
    const [left, top, right, bottom] = location.tileRect;
    suggestions.push({
      key: `location:${location.id}`,
      kind: "location",
      label,
      detail: location.anywhere
        ? "읽기 전용"
        : `타일 (${left}, ${top}) – (${right}, ${bottom})`,
      location,
    });
  }
  return suggestions.slice(0, MAP_MENTION_SUGGESTION_LIMIT);
}
