/**
 * The project sidebar's 사운드 tab: what the saved map holds, at a glance.
 * A row opens the center 사운드 tab on that sound, where it is played and
 * compared with its original.
 */
import { useTranslation } from "react-i18next";
import { ChevronRight, Music, RefreshCw } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { formatBytes, formatDuration } from "@/components/soundFormat";
import type { SoundPanelList } from "@/lib/ipc";

export interface SoundNavProps {
  list: SoundPanelList | null;
  loading: boolean;
  error: string | null;
  onRefresh(): void;
  onOpen(mpqPath: string): void;
}

export function SoundNav({ list, loading, error, onRefresh, onOpen }: SoundNavProps) {
  const { t } = useTranslation();
  return (
    <div className="flex h-full min-h-0 w-full flex-col">
      <div className="flex items-start gap-2 border-b border-border/60 p-3">
        <div className="min-w-0 flex-1 text-xs leading-relaxed text-muted-foreground">
          {list === null ? (
            t("sound.nav.intro")
          ) : (
            <>
              <p className="truncate font-medium text-foreground" title={list.mapName}>
                {list.mapName}
              </p>
              <p>
                {t("sound.summary", {
                  count: list.sounds.length + list.omitted,
                  size: formatBytes(list.totalBytes),
                })}
              </p>
            </>
          )}
        </div>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="size-8 shrink-0"
          aria-label={t("sound.refresh")}
          title={t("sound.refresh")}
          disabled={loading}
          onClick={onRefresh}
        >
          {loading ? <Spinner className="size-4" /> : <RefreshCw className="size-4" aria-hidden="true" />}
        </Button>
      </div>

      {error !== null && (
        <p role="alert" className="border-b border-border/60 p-3 text-xs leading-relaxed text-destructive">
          {error}
        </p>
      )}

      {list !== null && list.sounds.length === 0 && (
        <p className="p-3 text-xs text-muted-foreground">{t("sound.empty")}</p>
      )}

      {list !== null && list.sounds.length > 0 && (
        <ul className="min-h-0 flex-1 overflow-y-auto">
          {list.sounds.map((sound) => (
            <li key={sound.mpqPath}>
              <button
                type="button"
                onClick={() => onOpen(sound.mpqPath)}
                className="flex min-h-11 w-full items-center gap-2 border-b border-border/60 px-3 py-2 text-left hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:outline-none"
              >
                <Music className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm" title={sound.mpqPath}>
                    {sound.originals[0]?.name ?? sound.name}
                  </span>
                  <span className="block text-[11px] tabular-nums text-muted-foreground">
                    {formatDuration(sound.inMap?.durationMs)}
                    {" · "}
                    {sound.inMap ? formatBytes(sound.inMap.bytes) : t("sound.notInMap")}
                  </span>
                </span>
                {sound.originals.length === 0 && (
                  <Badge variant="outline" className="shrink-0 text-[10px]">
                    {t("sound.noOriginal")}
                  </Badge>
                )}
                <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
