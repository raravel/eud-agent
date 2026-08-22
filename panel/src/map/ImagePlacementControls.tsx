import { useEffect, useRef } from "react";
import {
  AlertTriangle,
  Check,
  Image as ImageIcon,
  LoaderCircle,
  X,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  clampImagePlacement,
  resizeImageFromHeight,
  resizeImageFromWidth,
} from "./imagePlacement";
import type {
  MapImageConversionReport,
  MapImageDimensions,
  MapImagePlacement,
} from "./mapProtocol";

export interface ImagePlacementControlsProps {
  fileName: string;
  sourceDimensions: MapImageDimensions;
  placement: MapImagePlacement;
  mapWidth: number;
  mapHeight: number;
  previewMode: "original" | "result";
  report?: MapImageConversionReport;
  previewFresh: boolean;
  previewLoading: boolean;
  confirming: boolean;
  error?: string;
  onPlacement(placement: MapImagePlacement, settled: boolean): void;
  onPreviewMode(mode: "original" | "result"): void;
  onConfirm(): void;
  onCancel(): void;
}

export function ImagePlacementControls({
  fileName,
  sourceDimensions,
  placement,
  mapWidth,
  mapHeight,
  previewMode,
  report,
  previewFresh,
  previewLoading,
  confirming,
  error,
  onPlacement,
  onPreviewMode,
  onConfirm,
  onCancel,
}: ImagePlacementControlsProps) {
  const debounceRef = useRef<number | null>(null);
  const pendingRef = useRef(placement);

  useEffect(
    () => () => {
      if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    },
    [],
  );

  function queueNumeric(next: MapImagePlacement) {
    pendingRef.current = next;
    onPlacement(next, false);
    if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    debounceRef.current = window.setTimeout(() => {
      debounceRef.current = null;
      onPlacement(pendingRef.current, true);
    }, 180);
  }

  function settleNumeric() {
    if (debounceRef.current !== null) window.clearTimeout(debounceRef.current);
    debounceRef.current = null;
    onPlacement(pendingRef.current, true);
  }

  const confirmDisabled =
    !report ||
    !previewFresh ||
    previewLoading ||
    confirming ||
    report.protectedConflicts > 0;

  return (
    <section
      aria-label="사진 지형 배치"
      className="w-full min-w-0 rounded-xl border border-border bg-card/95 p-3 shadow-2xl backdrop-blur"
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-cyan-500/30 bg-cyan-500/10">
          <ImageIcon className="size-4 text-cyan-300" aria-hidden="true" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold" title={fileName}>
            {fileName}
          </div>
          <div className="text-[11px] text-muted-foreground">
            원본 {sourceDimensions.width}×{sourceDimensions.height}px · 출력 {placement.width}×{placement.height}타일
          </div>
        </div>

        <div
          className="flex shrink-0 rounded-lg border border-border bg-background/70 p-1"
          aria-label="사진 미리보기 모드"
        >
          <Button
            type="button"
            size="sm"
            variant={previewMode === "original" ? "secondary" : "ghost"}
            aria-pressed={previewMode === "original"}
            onClick={() => onPreviewMode("original")}
          >
            원본 오버레이
          </Button>
          <Button
            type="button"
            size="sm"
            variant={previewMode === "result" ? "secondary" : "ghost"}
            aria-pressed={previewMode === "result"}
            disabled={!report}
            onClick={() => onPreviewMode("result")}
          >
            적용 결과
          </Button>
        </div>
      </div>

      <div className="mt-3 flex min-w-0 flex-wrap items-end gap-2">
        {(
          [
            ["X", "x", placement.x],
            ["Y", "y", placement.y],
            ["너비", "width", placement.width],
            ["높이", "height", placement.height],
          ] as const
        ).map(([label, field, value]) => (
          <label key={field} className="grid min-w-20 flex-1 gap-1 text-[11px] text-muted-foreground">
            <span>{label}</span>
            <Input
              type="number"
              min={field === "x" || field === "y" ? 0 : 1}
              max={
                field === "x"
                  ? mapWidth - placement.width
                  : field === "y"
                    ? mapHeight - placement.height
                    : field === "width"
                      ? Math.min(256, mapWidth)
                      : Math.min(256, mapHeight)
              }
              step={1}
              value={value}
              className="h-9 font-mono tabular-nums"
              onChange={(event) => {
                const numeric = Number(event.target.value);
                if (!Number.isFinite(numeric)) return;
                let next: MapImagePlacement;
                if (field === "width") {
                  next = resizeImageFromWidth(
                    placement,
                    numeric,
                    sourceDimensions,
                    mapWidth,
                    mapHeight,
                  );
                } else if (field === "height") {
                  next = resizeImageFromHeight(
                    placement,
                    numeric,
                    sourceDimensions,
                    mapWidth,
                    mapHeight,
                  );
                } else {
                  next = clampImagePlacement(
                    { ...placement, [field]: numeric },
                    mapWidth,
                    mapHeight,
                  );
                }
                queueNumeric(next);
              }}
              onBlur={settleNumeric}
              onKeyDown={(event) => {
                if (event.key === "Enter") settleNumeric();
              }}
            />
          </label>
        ))}

        <div className="flex min-w-0 flex-[3] flex-wrap items-center gap-1.5 text-[11px]">
          {previewLoading ? (
            <Badge variant="outline" className="gap-1">
              <LoaderCircle className="size-3 animate-spin" aria-hidden="true" />
              실제 타일 계산 중
            </Badge>
          ) : previewFresh ? (
            <Badge className="gap-1 bg-emerald-600 text-white">
              <Check className="size-3" aria-hidden="true" />
              최신 미리보기
            </Badge>
          ) : (
            <Badge variant="outline" className="gap-1 text-amber-300">
              <AlertTriangle className="size-3" aria-hidden="true" />
              미리보기 갱신 필요
            </Badge>
          )}
          {report && (
            <>
              <Badge variant="outline">변경 {report.changedCells.toLocaleString()}셀</Badge>
              <Badge variant="outline">고유 타일 {report.uniqueTileCount.toLocaleString()}</Badge>
              <Badge variant="outline">보행 변화 {report.walkabilityChangedCells.toLocaleString()}</Badge>
              <Badge variant="outline">고도 변화 {report.heightChangedCells.toLocaleString()}</Badge>
              <Badge variant={report.protectedConflicts > 0 ? "destructive" : "outline"}>
                protect 충돌 {report.protectedConflicts.toLocaleString()}
              </Badge>
            </>
          )}
        </div>
      </div>

      <div className="mt-2 flex min-w-0 flex-wrap items-center gap-2">
        <p className="min-w-0 flex-1 text-[11px] text-muted-foreground">
          본체 드래그 이동 · 모서리 드래그 비율 조절 · 방향키 1타일 · Shift+방향키 8타일
        </p>
        {error && (
          <p role="alert" className="min-w-0 flex-1 text-xs text-destructive">
            {error}
          </p>
        )}
        <Button
          type="button"
          size="sm"
          variant="ghost"
          disabled={confirming}
          onClick={onCancel}
        >
          <X className="size-4" aria-hidden="true" />
          취소
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={confirmDisabled}
          onClick={onConfirm}
        >
          {confirming && <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />}
          후보에 반영
        </Button>
      </div>
    </section>
  );
}
