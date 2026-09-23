/**
 * Pure rendering helpers for the harness document review. The CORE already
 * groups items (dat per objId, files by kind, flat for the rest); these helpers
 * only read a dat group's `properties[]` and derive a stable per-item key, since
 * a dat group carries NO item-level id.
 *
 * Contract (`@/lib/changeset`):
 *   export function datProperties(item: ChangesetItem): DatProperty[];
 *   export function itemKey(item: ChangesetItem): string;
 */
import { describe, it, expect } from "vitest";
import { datProperties, itemKey } from "./changeset";
import type { ChangesetItem } from "@/lib/ipc";

const fileItem: ChangesetItem = {
  category: "file",
  kind: "modified",
  path: "main.eps",
  id: "f1",
  seq: 3,
  diff: "--- a/main.eps\n+++ b/main.eps\n@@\n-old\n+new\n",
};

// REPRESENTATIVE: the core NEVER puts an item-level id/seq on a dat group —
// only on its properties[] (journal.changeset()). The ChangesetItem type
// requires id/seq, so the fixture casts to mirror the real id-less shape.
const datItem = {
  category: "dat",
  dat: "unit",
  objId: 76,
  properties: [
    { property: "MaxHp", old: "40", new: "80", id: "p1", seq: 0 },
    { property: "GasCost", old: "0", new: "25", id: "p2", seq: 1 },
  ],
} as unknown as ChangesetItem;

describe("datProperties", () => {
  it("reads every property row of a grouped dat item", () => {
    expect(datProperties(datItem).map((p) => p.property)).toEqual([
      "MaxHp",
      "GasCost",
    ]);
  });

  it("is empty for an item with no properties[]", () => {
    expect(datProperties(fileItem)).toEqual([]);
  });

  it("drops malformed property rows instead of rendering them", () => {
    const malformed = {
      category: "dat",
      dat: "unit",
      objId: 1,
      properties: [{ property: "MaxHp", old: 1, new: 2 }, null, "nope"],
    } as unknown as ChangesetItem;
    expect(datProperties(malformed)).toEqual([]);
  });
});

describe("itemKey", () => {
  it("uses the item-level id for a file/flat item", () => {
    expect(itemKey(fileItem)).toBe("f1");
  });

  it("falls back to the joined property ids for an id-less dat group", () => {
    // The core sends no item-level id; the key must be stable + non-undefined.
    expect(itemKey(datItem)).toBe("p1,p2");
  });

  it("gives DISTINCT keys to two id-less dat groups (no collision)", () => {
    const other = {
      category: "dat",
      dat: "unit",
      objId: 0,
      properties: [{ property: "MaxHp", old: "10", new: "20", id: "q1", seq: 4 }],
    } as unknown as ChangesetItem;
    expect(itemKey(datItem)).not.toBe(itemKey(other));
    expect(itemKey(other)).toBe("q1");
  });

  it("falls back to category/dat/objId when nothing carries an id", () => {
    const bare = {
      category: "dat",
      dat: "unit",
      objId: 7,
    } as unknown as ChangesetItem;
    expect(itemKey(bare)).toBe("dat-unit-7");
  });
});
