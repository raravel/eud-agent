import { useTranslation } from "react-i18next";
import { FileInput, FolderOpen, LoaderCircle, ShieldCheck, ShieldX } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  MapImportDestination,
  MapImportReference,
  MapImportSource,
} from "./importProtocol";

export interface MapImportToolbarProps {
  destination: MapImportDestination;
  source: MapImportSource | null;
  references: MapImportReference[];
  picking: boolean;
  stale: boolean;
  onPick(): void;
  onPickReference(name: string): void;
}

export function MapImportToolbar({
  destination,
  source,
  references,
  picking,
  stale,
  onPick,
  onPickReference,
}: MapImportToolbarProps) {
  const { t } = useTranslation();
  const compatible = source !== null && source.tileset === destination.tileset;
  return (
    <header className="flex min-w-0 flex-wrap items-center gap-3 border-b border-border bg-card/95 px-4 py-3">
      <Button type="button" onClick={onPick} disabled={picking} className="min-h-11">
        {picking ? (
          <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <FileInput className="size-4" aria-hidden="true" />
        )}
        {t("mapImport.toolbar.pick")}
      </Button>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <FolderOpen className="size-4" aria-hidden="true" />
        <span>references</span>
        <Select
          value={source?.referenceName ?? ""}
          disabled={picking || references.length === 0}
          onValueChange={onPickReference}
        >
          <SelectTrigger className="h-9 w-48" aria-label={t("mapImport.toolbar.referencesAria")}>
            <SelectValue
              placeholder={
                references.length === 0 ? t("mapImport.toolbar.noReferences") : t("mapImport.toolbar.openReference")
              }
            />
          </SelectTrigger>
          <SelectContent>
            {references.map((reference) => (
              <SelectItem key={reference.name} value={reference.name}>
                {reference.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-medium">
          {source?.displayName ?? t("mapImport.toolbar.chooseExternal")}
        </p>
        <p className="truncate text-xs text-muted-foreground">
          {source
            ? `${source.tileset} · ${source.width}×${source.height} · file ${source.fileSha256.slice(0, 10)} · CHK ${source.chkSha256.slice(0, 10)}${
                source.referenceName ? ` · references/${source.referenceName}` : ""
              }`
            : t("mapImport.toolbar.pinHint")}
        </p>
      </div>
      <div className="min-w-0 text-right text-xs text-muted-foreground">
        <p className="truncate">{t("mapImport.toolbar.destination", { name: destination.displayName })}</p>
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
            ? t("mapImport.toolbar.stale")
            : compatible
              ? t("mapImport.toolbar.sameTileset")
              : t("mapImport.toolbar.tilesetMismatch")}
        </Badge>
      )}
    </header>
  );
}
