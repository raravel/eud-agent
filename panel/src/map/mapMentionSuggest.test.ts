import { describe, expect, it } from "vitest";

import {
  MAP_MENTION_SUGGESTION_LIMIT,
  mapMentionSuggestions,
} from "./mapMentionSuggest";
import type { MapLocation, SavedSelection } from "./mapProtocol";

function selection(
  id: string,
  role: SavedSelection["role"],
  label: string,
): SavedSelection {
  return {
    id,
    label,
    role,
    sourceRevision: "r1",
    layers: ["terrain"],
    bounds: [0, 0, 4, 4],
    selectedCells: 16,
    rows: [],
    snapshotHash: `hash-${id}`,
  };
}

function location(id: number, name: string, anywhere = false): MapLocation {
  return {
    id,
    name,
    left: 0,
    top: 0,
    right: 64,
    bottom: 64,
    tileRect: [0, 0, 2, 2],
    elevationFlags: 0,
    anywhere,
  };
}

describe("mapMentionSuggestions", () => {
  const selections = [
    selection("s1", "target", "영역 1"),
    selection("s2", "protect", "기지"),
  ];
  const locations = [
    location(1, "Spawn"),
    location(7, ""),
    location(64, "Anywhere", true),
  ];

  it("lists saved selections before locations for an empty query", () => {
    const keys = mapMentionSuggestions("", selections, locations).map(
      (item) => item.key,
    );
    expect(keys).toEqual([
      "region:s1",
      "region:s2",
      "location:1",
      "location:7",
      "location:64",
    ]);
  });

  it("matches selections by role or label and locations by id or name", () => {
    expect(
      mapMentionSuggestions("tar", selections, locations).map((item) => item.label),
    ).toEqual(["target:영역 1"]);
    expect(
      mapMentionSuggestions("기지", selections, locations).map((item) => item.label),
    ).toEqual(["protect:기지"]);
    expect(
      mapMentionSuggestions("#7", selections, locations).map((item) => item.label),
    ).toEqual(["location:#7 이름 없음"]);
    expect(
      mapMentionSuggestions("spawn", selections, locations).map((item) => item.label),
    ).toEqual(["location:#1 Spawn"]);
    expect(mapMentionSuggestions("없는것", selections, locations)).toEqual([]);
  });

  it("describes read-only Anywhere and tile bounds in the detail", () => {
    const [anywhere] = mapMentionSuggestions("anywhere", selections, locations);
    expect(anywhere.detail).toBe("읽기 전용");
    const [spawn] = mapMentionSuggestions("spawn", selections, locations);
    expect(spawn.detail).toBe("타일 (0, 0) – (2, 2)");
    const [target] = mapMentionSuggestions("target", selections, locations);
    expect(target.detail).toBe("16셀 · terrain");
  });

  it("caps the rendered rows", () => {
    const many = Array.from({ length: 200 }, (_, index) =>
      location(index, `L${index}`),
    );
    expect(mapMentionSuggestions("", [], many)).toHaveLength(
      MAP_MENTION_SUGGESTION_LIMIT,
    );
  });
});
