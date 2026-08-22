import {
  CheckCircle2,
  GitCompareArrows,
  ImagePlus,
  History,
  LoaderCircle,
  Map as MapIcon,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  CandidateStateView,
  MapContextSnapshot,
  MapSourceProbe,
  MapView,
} from "./mapProtocol";

export interface MapToolbarProps {
  context: MapContextSnapshot;
  candidate: CandidateStateView;
  changedSource: MapSourceProbe | null;
  view: MapView;
  busy: boolean;
  reloadingSource: boolean;
  imagePlacementActive: boolean;
  liveDraftActive?: boolean;
  onView(view: MapView): void;
  onRevert(revision: number): void;
  onDiscard(): void;
  onApply(): void;
  onUndo(): void;
  onImagePlace(): void;
  onReloadSource(): void;
}

function savedTime(mtimeNs: string): string {
  try {
    const milliseconds = Number(BigInt(mtimeNs) / 1_000_000n);
    return new Intl.DateTimeFormat("ko-KR", {
      dateStyle: "short",
      timeStyle: "medium",
    }).format(new Date(milliseconds));
  } catch {
    return "시간 확인 불가";
  }
}

export function MapToolbar({
  context,
  candidate,
  changedSource,
  view,
  busy,
  reloadingSource,
  imagePlacementActive,
  liveDraftActive = false,
  onView,
  onRevert,
  onDiscard,
  onApply,
  onUndo,
  onImagePlace,
  onReloadSource,
}: MapToolbarProps) {
  const sourceName =
    candidate.baseline.sourcePath.split(/[\\/]/).at(-1) ?? candidate.baseline.sourcePath;
  const currentSourcePath = changedSource?.sourcePath ?? context.revision.sourcePath;
  const currentSourceMtime = changedSource?.mtimeNs ?? context.revision.mtimeNs;
  const currentSourceMatchesContext =
    changedSource === null ||
    (changedSource.projectId === context.revision.projectId &&
      changedSource.sourcePath === context.revision.sourcePath &&
      changedSource.mtimeNs === context.revision.mtimeNs &&
      changedSource.fileSize === context.sourceFileSize);
  return (
    <header className="flex min-w-0 flex-wrap items-center gap-3 border-b border-border bg-card/80 px-3 py-2 backdrop-blur">
      <div className="flex min-w-[18rem] flex-1 items-center gap-2">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-emerald-500/30 bg-emerald-500/15">
          <MapIcon className="size-5 text-emerald-300" aria-hidden="true" />
        </span>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <strong className="truncate text-sm">{sourceName}</strong>
            <Badge variant="outline">{candidate.baseline.tileset}</Badge>
            <Badge variant="outline">{candidate.baseline.width}×{candidate.baseline.height}</Badge>
          </div>
          <div className="flex min-w-0 gap-2 text-[11px] text-muted-foreground">
            <span className="shrink-0 font-medium text-foreground/80">후보 기준 원본</span>
            <span className="truncate" title={candidate.baseline.sourcePath}>
              {candidate.baseline.sourcePath}
            </span>
            <span className="shrink-0">저장 {savedTime(candidate.baseline.mtimeNs)}</span>
            <code className="shrink-0" title={candidate.baseline.fileSha256}>
              SHA {candidate.baseline.fileSha256.slice(0, 10)}
            </code>
          </div>
          {candidate.stale && (
            <div className="flex min-w-0 gap-2 text-[11px] text-destructive">
              <span className="shrink-0 font-medium">현재 OpenMapName</span>
              <span className="truncate" title={currentSourcePath}>
                {currentSourcePath}
              </span>
              <span className="shrink-0">저장 {savedTime(currentSourceMtime)}</span>
              {currentSourceMatchesContext && (
                <code className="shrink-0" title={context.revision.fileSha256}>
                  SHA {context.revision.fileSha256.slice(0, 10)}
                </code>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="flex items-center rounded-lg border border-border bg-background/50 p-1" aria-label="맵 비교 보기">
        {(["original", "candidate", "diff"] as const).map((mode) => (
          <Button
            key={mode}
            type="button"
            size="sm"
            variant={view === mode ? "secondary" : "ghost"}
            aria-pressed={view === mode}
            className="min-h-9"
            onClick={() => onView(mode)}
          >
            {mode === "original" ? "원본" : mode === "candidate" ? "후보" : "차이"}
          </Button>
        ))}
      </div>

      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <History className="size-4" aria-hidden="true" />
        <span>후보</span>
        <Select
          value={candidate.currentRevision.toString()}
          disabled={busy}
          onValueChange={(value) => onRevert(Number(value))}
        >
          <SelectTrigger size="sm" className="w-32" aria-label="후보 revision">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="0">r0 · 기준</SelectItem>
            {candidate.revisions.map((revision) => (
              <SelectItem key={revision.revision} value={revision.revision.toString()}>
                r{revision.revision} ← r{revision.parent}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      {liveDraftActive && (
        <Badge
          role="status"
          variant="secondary"
          className="gap-1 border border-cyan-400/30 bg-cyan-400/10 text-cyan-200"
        >
          <LoaderCircle
            className="size-3.5 animate-spin motion-reduce:animate-none"
            aria-hidden="true"
          />
          수정 중 미리보기 · 미확정
        </Badge>
      )}

      {candidate.stale ? (
        <Badge variant="destructive" className="gap-1">
          <ShieldAlert className="size-3.5" aria-hidden="true" />
          원본 변경됨 · Apply 차단
        </Badge>
      ) : candidate.canApply ? (
        <Badge className="gap-1 bg-emerald-600 text-white">
          <CheckCircle2 className="size-3.5" aria-hidden="true" />
          검증 통과
        </Badge>
      ) : (
        <Badge variant="outline" className="gap-1">
          <GitCompareArrows className="size-3.5" aria-hidden="true" />
          후보 없음
        </Badge>
      )}

      <div className="ml-auto flex items-center gap-2">
        {candidate.stale && (
          <Button
            type="button"
            size="sm"
            className="gap-1.5"
            disabled={busy}
            title="기존 작업은 히스토리에 보존됩니다."
            onClick={onReloadSource}
          >
            {reloadingSource ? (
              <LoaderCircle
                className="size-4 animate-spin motion-reduce:animate-none"
                aria-hidden="true"
              />
            ) : (
              <RefreshCw className="size-4" aria-hidden="true" />
            )}
            {reloadingSource ? "새 작업 여는 중…" : "변경된 원본으로 새 작업"}
          </Button>
        )}
        <Button
          type="button"
          size="sm"
          variant={imagePlacementActive ? "secondary" : "outline"}
          disabled={busy || candidate.stale}
          aria-pressed={imagePlacementActive}
          onClick={onImagePlace}
        >
          <ImagePlus className="size-4" aria-hidden="true" />
          사진 배치
        </Button>
        <Button type="button" size="sm" variant="outline" disabled={busy || !candidate.canUndo} onClick={onUndo}>
          <RotateCcw className="size-4" aria-hidden="true" />
          마지막 적용 취소
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy || candidate.currentRevision === 0}
          title="현재 후보 전체를 취소하고 기준 맵으로 돌아가기"
          onClick={onDiscard}
        >
          <X className="size-4" aria-hidden="true" />
          후보 취소
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={busy || candidate.stale || !candidate.canApply}
          onClick={onApply}
        >
          전체 Apply
        </Button>
      </div>
    </header>
  );
}
