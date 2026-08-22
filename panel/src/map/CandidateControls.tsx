import { AlertTriangle, CheckCircle2, GitCompareArrows } from "lucide-react";

import type {
  CandidateStateView,
  LayerDiffCount,
  MapDiff,
  MapDiffDetails,
} from "./mapProtocol";

function emptyCount(): LayerDiffCount {
  return { added: 0, removed: 0, moved: 0, changed: 0 };
}

export function cumulativeDiff(
  details: MapDiffDetails,
  authority: MapDiff,
): MapDiff {
  const diff: MapDiff = {
    terrainCells: authority.terrainCells,
    terrainBounds: authority.terrainBounds,
    units: { ...authority.units },
    buildings: { ...authority.buildings },
    doodads: { ...authority.doodads },
    sprites: { ...authority.sprites },
    locations: { ...authority.locations },
    outsideTarget: authority.outsideTarget,
    protected: authority.protected,
    unsupportedSectionChanges: [...authority.unsupportedSectionChanges],
  };
  if (details.terrainRows.length > 0) {
    diff.terrainCells = 0;
    diff.terrainBounds = undefined;
    let left = Number.POSITIVE_INFINITY;
    let top = Number.POSITIVE_INFINITY;
    let right = 0;
    let bottom = 0;
    for (const row of details.terrainRows) {
      for (const [spanLeft, spanRight] of row.spans) {
        diff.terrainCells += spanRight - spanLeft;
        left = Math.min(left, spanLeft);
        top = Math.min(top, row.y);
        right = Math.max(right, spanRight);
        bottom = Math.max(bottom, row.y + 1);
      }
    }
    if (diff.terrainCells > 0) {
      diff.terrainBounds = { left, top, right, bottom };
    }
  }
  if (details.markers.length > 0) {
    diff.units = emptyCount();
    diff.buildings = emptyCount();
    diff.doodads = emptyCount();
    diff.sprites = emptyCount();
    diff.locations = emptyCount();
    for (const marker of details.markers) {
      const count =
        marker.layer === "units"
          ? diff.units
          : marker.layer === "buildings"
            ? diff.buildings
            : marker.layer === "doodads"
              ? diff.doodads
              : marker.layer === "sprites"
                ? diff.sprites
                : marker.layer === "locations"
                  ? diff.locations
                  : undefined;
      if (count) count[marker.change] += 1;
    }
  }
  return diff;
}


export function CandidateControls({
  candidate,
  details,
}: {
  candidate: CandidateStateView;
  details: MapDiffDetails;
}) {
  const revision = candidate.revisions.find(
    (item) => item.revision === candidate.currentRevision,
  );
  if (!revision) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <GitCompareArrows className="size-4" aria-hidden="true" />
        원본과 동일한 기준 revision
      </div>
    );
  }
  const diff = cumulativeDiff(details, revision.diff);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
      {revision.verification.valid ? (
        <span className="flex items-center gap-1 text-emerald-300">
          <CheckCircle2 className="size-3.5" aria-hidden="true" />검증 통과
        </span>
      ) : (
        <span className="flex items-center gap-1 text-destructive">
          <AlertTriangle className="size-3.5" aria-hidden="true" />검증 실패
        </span>
      )}
      <span>
        terrain {diff.terrainCells.toLocaleString()} cells
        {diff.terrainBounds
          ? ` (${diff.terrainBounds.left},${diff.terrainBounds.top})–(${diff.terrainBounds.right},${diff.terrainBounds.bottom})`
          : ""}
      </span>
      <span>unit +{diff.units.added}/-{diff.units.removed}/↔{diff.units.moved}/~{diff.units.changed}</span>
      <span>building +{diff.buildings.added}/-{diff.buildings.removed}/↔{diff.buildings.moved}/~{diff.buildings.changed}</span>
      <span>doodad +{diff.doodads.added}/-{diff.doodads.removed}/↔{diff.doodads.moved}/~{diff.doodads.changed}</span>
      <span>sprite +{diff.sprites.added}/-{diff.sprites.removed}/↔{diff.sprites.moved}/~{diff.sprites.changed}</span>
      <span>location +{diff.locations.added}/-{diff.locations.removed}/↔{diff.locations.moved}/~{diff.locations.changed}</span>
      <span className={diff.outsideTarget ? "text-destructive" : "text-emerald-300"}>target 밖 {diff.outsideTarget}</span>
      <span className={diff.protected ? "text-destructive" : "text-emerald-300"}>protect 침범 {diff.protected}</span>
      <span className={diff.unsupportedSectionChanges.length ? "text-destructive" : "text-emerald-300"}>
        unsupported {diff.unsupportedSectionChanges.length}
      </span>
    </div>
  );
}
