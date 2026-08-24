import { AlertTriangle, Check, CopyPlus, LoaderCircle, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type {
  SavedSelection,
  StampCollisionPolicy,
  StampDestination,
  StampLayerCounts,
  StampPlacementReport,
} from "./mapProtocol";

export interface StampPlacementControlsProps {
  selection: SavedSelection;
  sourceKind: "candidateSelection" | "imported";
  destination: StampDestination;
  mapWidth: number;
  mapHeight: number;
  report?: StampPlacementReport;
  previewFresh: boolean;
  previewLoading: boolean;
  confirming: boolean;
  error?: string;
  onDestination(destination: StampDestination): void;
  onConfirm(policy: StampCollisionPolicy): void;
  onCancel(): void;
}

function countTotal(counts: StampLayerCounts): number {
  return counts.units + counts.buildings + counts.doodads + counts.sprites + counts.locations;
}

export function StampPlacementControls({
  selection,
  sourceKind,
  destination,
  mapWidth,
  mapHeight,
  report,
  previewFresh,
  previewLoading,
  confirming,
  error,
  onDestination,
  onConfirm,
  onCancel,
}: StampPlacementControlsProps) {
  const width = selection.bounds.right - selection.bounds.left;
  const height = selection.bounds.bottom - selection.bounds.top;
  const collisionCount = report ? countTotal(report.collisions) : 0;
  const partialCollisionCount = report ? countTotal(report.partialCollisions) : 0;
  const baseBlocked =
    !report ||
    !previewFresh ||
    previewLoading ||
    confirming ||
    report.outsideAuthorityCells > 0 ||
    report.protectedCells > 0;
  const mergeBlocked =
    baseBlocked ||
    (report !== undefined &&
      report.requiredLocationSlots > report.availableLocationSlots);
  const replaceBlocked =
    baseBlocked ||
    partialCollisionCount > 0 ||
    (report !== undefined &&
      report.requiredLocationSlots >
        report.availableLocationSlots + report.collisions.locations);

  const updateCoordinate = (field: "x" | "y", value: number) => {
    if (!Number.isFinite(value)) return;
    onDestination({
      ...destination,
      [field]: Math.max(
        0,
        Math.min(
          field === "x" ? mapWidth - width : mapHeight - height,
          Math.trunc(value),
        ),
      ),
    });
  };

  return (
    <section
      aria-label="영역 스탬프 배치"
      className="w-full min-w-0 rounded-xl border border-border bg-card/95 p-3 shadow-2xl backdrop-blur"
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-cyan-500/30 bg-cyan-500/10">
          <CopyPlus className="size-4 text-cyan-300" aria-hidden="true" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold" title={selection.label}>
            {selection.label}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {width}×{height}타일 · {selection.selectedCells.toLocaleString()}셀 · {sourceKind === "imported" ? "외부 맵 고정 스냅샷" : "현재 후보 내용"}
          </div>
        </div>
        <Badge variant="outline">
          {selection.layers.length === 0 ? "전체 레이어" : selection.layers.join(" · ")}
        </Badge>
      </div>

      <div className="mt-3 flex min-w-0 flex-wrap items-end gap-2">
        {(["x", "y"] as const).map((field) => (
          <label key={field} className="grid min-w-24 gap-1 text-[11px] text-muted-foreground">
            <span>{field.toUpperCase()}</span>
            <Input
              type="number"
              min={0}
              max={field === "x" ? mapWidth - width : mapHeight - height}
              step={1}
              value={destination[field]}
              className="h-9 font-mono tabular-nums"
              onChange={(event) => updateCoordinate(field, Number(event.target.value))}
            />
          </label>
        ))}

        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5 text-[11px]">
          {previewLoading ? (
            <Badge variant="outline" className="gap-1">
              <LoaderCircle className="size-3 animate-spin" aria-hidden="true" />
              충돌 검사 중
            </Badge>
          ) : previewFresh ? (
            <Badge className="gap-1 bg-emerald-600 text-white">
              <Check className="size-3" aria-hidden="true" />
              배치 검사 완료
            </Badge>
          ) : (
            <Badge variant="outline" className="gap-1 text-amber-300">
              <AlertTriangle className="size-3" aria-hidden="true" />
              배치 검사 필요
            </Badge>
          )}
          {report && (
            <>
              <Badge variant="outline">
                지형 {(report.terrainCellsPerDestination * report.destinations.length).toLocaleString()}셀
              </Badge>
              <Badge variant={collisionCount > 0 ? "destructive" : "outline"}>
                개체·로케이션 충돌 {collisionCount.toLocaleString()}
              </Badge>
              <Badge variant={report.protectedCells > 0 ? "destructive" : "outline"}>
                protect {report.protectedCells.toLocaleString()}
              </Badge>
            </>
          )}
        </div>
      </div>

      {collisionCount > 0 && (
        <div role="alert" className="mt-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-100">
          목적지에 선택 레이어의 개체 또는 로케이션이 있습니다. 병합, 교체 또는 취소를 선택하세요.
          {partialCollisionCount > 0 && (
            <span className="mt-1 block text-[11px] text-amber-200">
              경계에 걸친 항목 {partialCollisionCount}개가 있어 교체는 사용할 수 없습니다.
            </span>
          )}
        </div>
      )}

      <div className="mt-2 flex min-w-0 flex-wrap items-center gap-2">
        <p className="min-w-0 flex-1 text-[11px] text-muted-foreground">
          캔버스 클릭으로 좌상단 지정 · 방향키 1타일 · Shift+방향키 8타일 · ISOM 보정 없음
        </p>
        {error && (
          <p role="alert" className="min-w-0 flex-1 text-xs text-destructive">
            {error}
          </p>
        )}
        <Button type="button" size="sm" variant="ghost" disabled={confirming} onClick={onCancel}>
          <X className="size-4" aria-hidden="true" />
          취소
        </Button>
        {collisionCount > 0 ? (
          <>
            <Button type="button" size="sm" variant="secondary" disabled={mergeBlocked} onClick={() => onConfirm("merge")}>
              {confirming && <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />}
              병합
            </Button>
            <Button type="button" size="sm" variant="destructive" disabled={replaceBlocked} onClick={() => onConfirm("replace")}>
              교체
            </Button>
          </>
        ) : (
          <Button type="button" size="sm" disabled={mergeBlocked} onClick={() => onConfirm("merge")}>
            {confirming && <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />}
            후보에 배치
          </Button>
        )}
      </div>
    </section>
  );
}
