import { FileInput, LoaderCircle, ShieldCheck, ShieldX } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type {
  MapImportDestination,
  MapImportSource,
} from "./importProtocol";

export interface MapImportToolbarProps {
  destination: MapImportDestination;
  source: MapImportSource | null;
  picking: boolean;
  stale: boolean;
  onPick(): void;
}

export function MapImportToolbar({
  destination,
  source,
  picking,
  stale,
  onPick,
}: MapImportToolbarProps) {
  const compatible = source !== null && source.tileset === destination.tileset;
  return (
    <header className="flex min-w-0 flex-wrap items-center gap-3 border-b border-border bg-card/95 px-4 py-3">
      <Button type="button" onClick={onPick} disabled={picking} className="min-h-11">
        {picking ? (
          <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <FileInput className="size-4" aria-hidden="true" />
        )}
        SCX/SCM 선택
      </Button>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">
          {source?.displayName ?? "외부 맵을 선택하세요"}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          {source
            ? `${source.tileset} · ${source.width}×${source.height} · file ${source.fileSha256.slice(0, 10)} · CHK ${source.chkSha256.slice(0, 10)}`
            : `.scx/.scm 내부 staredit\\scenario.chk만 고정합니다.`}
        </p>
      </div>
      <div className="min-w-0 text-right text-xs text-muted-foreground">
        <p className="truncate">대상 {destination.displayName}</p>
        <p>{destination.tileset} · {destination.width}×{destination.height}</p>
      </div>
      {source && (
        <Badge
          variant={compatible && !stale ? "secondary" : "destructive"}
          className="gap-1"
        >
          {compatible && !stale ? (
            <ShieldCheck className="size-3.5" aria-hidden="true" />
          ) : (
            <ShieldX className="size-3.5" aria-hidden="true" />
          )}
          {stale
            ? "대상 변경됨"
            : compatible
              ? "같은 타일셋"
              : "타일셋 불일치"}
        </Badge>
      )}
    </header>
  );
}
