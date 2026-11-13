import i18n from "@/lib/i18n";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  GitCompareArrows,
  ImagePlus,
  FolderInput,
  History,
  LoaderCircle,
  Map as MapIcon,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  SlidersHorizontal,
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
import { MapExportMenu } from "./MapExportMenu";
import type { CandidateStateView, MapView } from "./mapProtocol";

export interface MapToolbarProps {
  candidate: CandidateStateView;
  view: MapView;
  busy: boolean;
  imagePlacementActive: boolean;
  liveDraftActive?: boolean;
  /** The request whose live draft the canvas shows, so the export matches it. */
  draftRequestId?: string;
  onView(view: MapView): void;
  onRevert(revision: number): void;
  onDiscard(): void;
  onApply(): void;
  onUndo(): void;
  onImagePlace(): void;
  onMapImport(): void;
  onProperties(): void;
}

function savedTime(mtimeNs: string): string {
  try {
    const milliseconds = Number(BigInt(mtimeNs) / 1_000_000n);
    return new Intl.DateTimeFormat(i18n.language === "en" ? "en-US" : "ko-KR", {
      dateStyle: "short",
      timeStyle: "medium",
    }).format(new Date(milliseconds));
  } catch {
    return i18n.t("map.toolbar.timeUnknown");
  }
}

export function MapToolbar({
  candidate,
  view,
  busy,
  imagePlacementActive,
  liveDraftActive = false,
  draftRequestId,
  onView,
  onRevert,
  onDiscard,
  onApply,
  onUndo,
  onImagePlace,
  onMapImport,
  onProperties,
}: MapToolbarProps) {
  const { t } = useTranslation();
  const sourceName =
    candidate.baseline.sourcePath.split(/[\\/]/).at(-1) ?? candidate.baseline.sourcePath;
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
            <span className="shrink-0 font-medium text-foreground/80">{t("map.toolbar.source")}</span>
            <span className="truncate" title={candidate.baseline.sourcePath}>
              {candidate.baseline.sourcePath}
            </span>
            <span className="shrink-0">{t("map.toolbar.saved", { time: savedTime(candidate.baseline.mtimeNs) })}</span>
            <code className="shrink-0" title={candidate.baseline.fileSha256}>
              SHA {candidate.baseline.fileSha256.slice(0, 10)}
            </code>
          </div>
        </div>
      </div>

      <div className="flex items-center rounded-lg border border-border bg-background/50 p-1" aria-label={t("map.toolbar.compareAria")}>
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
            {t(`map.view.${mode}`)}
          </Button>
        ))}
      </div>

      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <History className="size-4" aria-hidden="true" />
        <span>{t("map.toolbar.candidate")}</span>
        <Select
          value={candidate.currentRevision.toString()}
          disabled={busy}
          onValueChange={(value) => onRevert(Number(value))}
        >
          <SelectTrigger className="h-9 w-32" aria-label={t("map.toolbar.revisionAria")}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="0">{t("map.toolbar.baseline")}</SelectItem>
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
          {t("map.toolbar.livePreview")}
        </Badge>
      )}

      {candidate.stale ? (
        <Badge
          variant="outline"
          className="gap-1"
          title={t("map.toolbar.staleTitle")}
        >
          <RefreshCw className="size-3.5" aria-hidden="true" />
          {t("map.toolbar.stale")}
        </Badge>
      ) : candidate.sourceDiverged ? (
        <Badge
          variant="destructive"
          className="gap-1"
          title={t("map.toolbar.divergedTitle")}
        >
          <ShieldAlert className="size-3.5" aria-hidden="true" />
          {t("map.toolbar.diverged")}
        </Badge>
      ) : candidate.canApply ? (
        <Badge className="gap-1 bg-emerald-600 text-white">
          <CheckCircle2 className="size-3.5" aria-hidden="true" />
          {t("map.toolbar.verified")}
        </Badge>
      ) : (
        <Badge variant="outline" className="gap-1">
          <GitCompareArrows className="size-3.5" aria-hidden="true" />
          {t("map.toolbar.noCandidate")}
        </Badge>
      )}

      <div className="ml-auto flex items-center gap-2">
        <MapExportMenu
          command={{
            sessionId: candidate.sessionId,
            view: draftRequestId === undefined ? view : "draft",
            requestId: draftRequestId,
          }}
        />
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy}
          className="h-9"
          title={t("map.toolbar.propertiesTitle")}
          onClick={onProperties}
        >
          <SlidersHorizontal className="size-4" aria-hidden="true" />
          {t("map.toolbar.properties")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy}
          className="h-9"
          onClick={onMapImport}
        >
          <FolderInput className="size-4" aria-hidden="true" />
          {t("map.toolbar.import")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant={imagePlacementActive ? "secondary" : "outline"}
          disabled={busy || candidate.stale}
          className="h-9"
          aria-pressed={imagePlacementActive}
          onClick={onImagePlace}
        >
          <ImagePlus className="size-4" aria-hidden="true" />
          {t("map.toolbar.placePhoto")}
        </Button>
        <Button type="button" size="sm" className="h-9" variant="outline" disabled={busy || !candidate.canUndo} onClick={onUndo}>
          <RotateCcw className="size-4" aria-hidden="true" />
          {t("map.toolbar.undo")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={busy || candidate.currentRevision === 0}
          className="h-9"
          title={t("map.toolbar.discardTitle")}
          onClick={onDiscard}
        >
          <X className="size-4" aria-hidden="true" />
          {t("map.toolbar.discard")}
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={busy || candidate.stale || !candidate.canApply}
          className="h-9"
          onClick={onApply}
        >
          {t("map.toolbar.applyAll")}
        </Button>
      </div>
    </header>
  );
}
