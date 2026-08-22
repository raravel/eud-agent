import type { MapObjectItem, MapObjectKind, TileRect } from "./mapProtocol";

export interface SpatialObject {
  id: string;
  kind: MapObjectKind | "location";
  bounds: TileRect;
  z: number;
  item: MapObjectItem;
}

export class MapSpatialIndex {
  private readonly buckets = new Map<string, SpatialObject[]>();

  constructor(objects: SpatialObject[]) {
    for (const object of objects) {
      for (let y = object.bounds.top; y < object.bounds.bottom; y += 1) {
        for (let x = object.bounds.left; x < object.bounds.right; x += 1) {
          const key = `${x},${y}`;
          const bucket = this.buckets.get(key) ?? [];
          bucket.push(object);
          this.buckets.set(key, bucket);
        }
      }
    }
    for (const bucket of this.buckets.values()) {
      bucket.sort((left, right) => right.z - left.z || left.id.localeCompare(right.id));
    }
  }

  hit(tileX: number, tileY: number): SpatialObject[] {
    return this.buckets.get(`${tileX},${tileY}`) ?? [];
  }

  cycle(tileX: number, tileY: number, previousId?: string): SpatialObject | null {
    const hits = this.hit(tileX, tileY);
    if (hits.length === 0) return null;
    const previous = previousId
      ? hits.findIndex((object) => object.id === previousId)
      : -1;
    return hits[(previous + 1) % hits.length];
  }
}

export function spatialObjectsFromPages(pages: MapObjectItem[]): SpatialObject[] {
  const objects: SpatialObject[] = [];
  for (const item of pages) {
    if (item.location) {
      const [left, top, right, bottom] = item.location.tileRect;
      objects.push({
        id: `location:${item.location.id}`,
        kind: "location",
        bounds: {
          left: Math.min(left, right),
          top: Math.min(top, bottom),
          right: Math.max(left + 1, right),
          bottom: Math.max(top + 1, bottom),
        },
        z: 10,
        item,
      });
      continue;
    }
    if (!item.object || !item.objectRef) continue;
    const object = item.object;
    const halfWidth = item.objectRef.kind === "building" ? 2 : 1;
    const halfHeight = item.objectRef.kind === "building" ? 2 : 1;
    objects.push({
      id: `${item.objectRef.kind}:${item.objectRef.ordinal}:${item.objectRef.semanticFingerprint}`,
      kind: item.objectRef.kind,
      bounds: {
        left: Math.max(0, object.tileX - halfWidth),
        top: Math.max(0, object.tileY - halfHeight),
        right: object.tileX + halfWidth + 1,
        bottom: object.tileY + halfHeight + 1,
      },
      z:
        item.objectRef.kind === "building" || item.objectRef.kind === "unit"
          ? 40
          : item.objectRef.kind === "sprite"
            ? 30
            : 20,
      item,
    });
  }
  return objects;
}
