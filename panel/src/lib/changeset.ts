/**
 * Pure changeset rendering/decision helpers (features/06 ## Behaviors →
 * Changeset review).
 *
 * The SERVER assembles the changeset already grouped (journal.changeset()): dat
 * writes are GROUPED per (dat, objId) with a `properties[]` list, files are one
 * item per kind, and the rest (settings/plugins/main/tbl/req/btn) are flat.
 * Crucially, the per-item DECISION TARGET id differs by shape:
 *   - a grouped dat item carries NO real journal id — the ids live on each
 *     `properties[].id`, so a whole-group decision targets every property id;
 *   - every other item carries a single item-level `id`.
 *
 * These helpers derive (a) the decision-target ids for the
 * `changeset_decision{ids}` payload and (b) the aggregate decision STATE of an
 * item from the per-id decisions map, so ChangesetView stays a thin renderer.
 */
import type { ChangesetItem } from "@/lib/ipc";
import type { ItemDecision } from "@/state/store";

/** Aggregate decision state of a rendered changeset item. */
export type ItemState =
  | "undecided"
  | "accepted"
  | "rejected"
  | "failed"
  | "mixed";

/** One property row of a grouped dat item. */
export interface DatProperty {
  property: string;
  old: unknown;
  new: unknown;
  id: string;
  seq: number;
}

/** Read a dat item's `properties[]` defensively (empty when absent/malformed). */
export function datProperties(item: ChangesetItem): DatProperty[] {
  const props = item.properties;
  if (!Array.isArray(props)) return [];
  return props.filter(
    (p): p is DatProperty =>
      typeof p === "object" && p !== null && typeof (p as DatProperty).id === "string",
  );
}

/**
 * The decision-target ids for an item: every property id for a grouped dat
 * item, otherwise the single item-level id.
 */
export function itemIds(item: ChangesetItem): string[] {
  if (item.category === "dat") {
    return datProperties(item).map((p) => p.id);
  }
  return [item.id];
}

/**
 * A STABLE per-item key for React keying + `data-testid`. A dat group carries
 * NO item-level `id` (the server only puts ids on its properties), so keying on
 * `item.id` collides (`undefined`) across multiple dat groups — use the joined
 * property ids as the stable identity instead. File/flat items keep their own
 * `id`. The join of decision-target ids is unique per rendered item and is the
 * SAME identity the decision payload targets.
 */
export function itemKey(item: ChangesetItem): string {
  if (typeof item.id === "string" && item.id !== "") return item.id;
  const ids = itemIds(item);
  if (ids.length > 0) return ids.join(",");
  // Defensive last resort (an item with neither an id nor any property id).
  return `${item.category}-${asKeyPart(item.dat)}-${asKeyPart(item.objId)}`;
}

function asKeyPart(value: unknown): string {
  return value === undefined || value === null ? "" : String(value);
}

/**
 * Aggregate an item's decision state from the per-id decisions map. A single-id
 * item mirrors its id's decision (or undecided). A dat group is:
 *   - undecided if ANY property is still undecided;
 *   - failed if any property failed (failure dominates so it surfaces inline);
 *   - accepted / rejected if every property shares that outcome;
 *   - mixed otherwise (some accepted, some rejected).
 */
export function itemState(
  item: ChangesetItem,
  decisions: Record<string, ItemDecision>,
): ItemState {
  const ids = itemIds(item);
  if (ids.length === 0) return "undecided";
  const outcomes = ids.map((id) => decisions[id]);
  if (outcomes.some((o) => o === undefined)) return "undecided";
  if (outcomes.some((o) => o === "failed")) return "failed";
  const allAccepted = outcomes.every((o) => o === "accepted");
  if (allAccepted) return "accepted";
  const allRejected = outcomes.every((o) => o === "rejected");
  if (allRejected) return "rejected";
  return "mixed";
}
