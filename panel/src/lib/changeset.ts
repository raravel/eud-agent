/**
 * Pure rendering helpers for the HARNESS document review, the one changeset
 * surface that survived the git cutover ({@link HarnessChangesetView}).
 *
 * The CORE assembles a changeset already grouped (journal.changeset()): dat
 * writes are GROUPED per (dat, objId) with a `properties[]` list, files are one
 * item per kind, and the rest (settings/plugins/main/tbl/req/btn) are flat.
 * Crucially, a grouped dat item carries NO item-level `id` — the ids live on
 * each `properties[].id` — so keying on `item.id` collides (`undefined`) across
 * multiple dat groups. {@link itemKey} derives a stable identity either way.
 */
import type { ChangesetItem } from "@/lib/ipc";

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
 * A STABLE per-item key for React keying + `data-testid`: the item-level `id`
 * when there is one, otherwise the joined property ids of a dat group, which
 * is unique per rendered item.
 */
export function itemKey(item: ChangesetItem): string {
  if (typeof item.id === "string" && item.id !== "") return item.id;
  const ids = datProperties(item).map((p) => p.id);
  if (ids.length > 0) return ids.join(",");
  // Defensive last resort (an item with neither an id nor any property id).
  return `${item.category}-${asKeyPart(item.dat)}-${asKeyPart(item.objId)}`;
}

function asKeyPart(value: unknown): string {
  return value === undefined || value === null ? "" : String(value);
}
