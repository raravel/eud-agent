/**
 * Pure changeset rendering/decision helpers (features/06 ## Behaviors →
 * Changeset review). The SERVER already groups items (dat per objId, files by
 * kind, flat for the rest); these helpers only derive, per rendered item:
 *   - the decision-target ids (the `changeset_decision{ids}` payload) — a dat
 *     group's ids live on its `properties[]`, every other item carries a single
 *     item-level `id`;
 *   - the aggregate decision STATE of an item from the per-id decisions map
 *     (accepted / rejected / failed / undecided / mixed).
 *
 * Contract (Step B implements `@/lib/changeset`):
 *   export function itemIds(item: ChangesetItem): string[];
 *   export type ItemState =
 *     "undecided" | "accepted" | "rejected" | "failed" | "mixed";
 *   export function itemState(
 *     item: ChangesetItem,
 *     decisions: Record<string, ItemDecision>,
 *   ): ItemState;
 */
import { describe, it, expect } from "vitest";
import { itemIds, itemKey, itemState } from "./changeset";
import type { ChangesetItem } from "@/lib/ipc";

const fileItem: ChangesetItem = {
  category: "file",
  kind: "modified",
  path: "main.eps",
  id: "f1",
  seq: 3,
  diff: "--- a/main.eps\n+++ b/main.eps\n@@\n-old\n+new\n",
};

// REPRESENTATIVE: the server NEVER puts an item-level id/seq on a dat group —
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

describe("itemIds", () => {
  it("returns the single item-level id for a file item", () => {
    expect(itemIds(fileItem)).toEqual(["f1"]);
  });

  it("returns every property id for a grouped dat item", () => {
    expect(itemIds(datItem)).toEqual(["p1", "p2"]);
  });

  it("returns the single id for a flat item (settings/plugin/main)", () => {
    const flat: ChangesetItem = {
      category: "settings",
      tool: "settings_set",
      target: "trigger_editor",
      old: { value: "a" },
      new: { value: "b" },
      id: "s1",
      seq: 5,
    };
    expect(itemIds(flat)).toEqual(["s1"]);
  });
});

describe("itemKey", () => {
  it("uses the item-level id for a file/flat item", () => {
    expect(itemKey(fileItem)).toBe("f1");
  });

  it("falls back to the joined property ids for an id-less dat group", () => {
    // The server sends no item-level id; the key must be stable + non-undefined.
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
});

describe("itemState", () => {
  it("is undecided when no id is decided", () => {
    expect(itemState(fileItem, {})).toBe("undecided");
  });

  it("is accepted when the file id is accepted", () => {
    expect(itemState(fileItem, { f1: "accepted" })).toBe("accepted");
  });

  it("is rejected when the file id is rejected (되돌림)", () => {
    expect(itemState(fileItem, { f1: "rejected" })).toBe("rejected");
  });

  it("is failed when the file id failed (rollback failure)", () => {
    expect(itemState(fileItem, { f1: "failed" })).toBe("failed");
  });

  it("is accepted only when EVERY dat property is accepted", () => {
    expect(itemState(datItem, { p1: "accepted", p2: "accepted" })).toBe(
      "accepted",
    );
  });

  it("is undecided when a dat group has a still-undecided property", () => {
    expect(itemState(datItem, { p1: "accepted" })).toBe("undecided");
  });

  it("surfaces failure across a dat group (failed dominates)", () => {
    expect(itemState(datItem, { p1: "accepted", p2: "failed" })).toBe("failed");
  });

  it("is mixed when a dat group has both accepted and rejected (no failure)", () => {
    expect(itemState(datItem, { p1: "accepted", p2: "rejected" })).toBe(
      "mixed",
    );
  });
});
