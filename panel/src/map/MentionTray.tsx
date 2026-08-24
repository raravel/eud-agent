import {
  Box,
  CopyPlus,
  Focus,
  MapPinned,
  Mountain,
  PackagePlus,
  Trash2,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { MentionChip } from "./mapProtocol";

export interface MentionTrayProps {
  chips: MentionChip[];
  selectedId?: string;
  onSelect(id: string): void;
  onRemove(id: string): void;
  onFind(id: string): void;
  onHighlight(id?: string): void;
}

export function MentionTray({
  chips,
  selectedId,
  onSelect,
  onRemove,
  onFind,
  onHighlight,
}: MentionTrayProps) {
  if (chips.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-border p-3 text-xs text-muted-foreground">
        선택 영역, 캔버스 개체, 팔레트 타입을 멘션으로 추가하세요.
      </div>
    );
  }
  return (
    <div className="flex flex-wrap gap-2" aria-label="프롬프트 멘션">
      {chips.map((chip) => {
        const Icon =
          chip.mention.kind === "region"
            ? MapPinned
            : chip.mention.kind === "stamp" || chip.mention.kind === "importedStamp"
              ? CopyPlus
              : chip.mention.kind === "object"
                ? Box
                : chip.mention.kind === "location"
                  ? Focus
                  : chip.mention.entry.layer === "terrain"
                    ? Mountain
                    : PackagePlus;
        return (
          <div
            key={chip.id}
            className={cn(
              "flex min-h-10 items-center rounded-full border bg-background/70 pl-3",
              selectedId === chip.id ? "border-primary ring-1 ring-primary" : "border-border",
              chip.stale && "border-destructive text-destructive-foreground",
            )}
            onMouseEnter={() => onHighlight(chip.id)}
            onMouseLeave={() => onHighlight()}
            onFocus={() => onHighlight(chip.id)}
            onBlur={(event) => {
              if (!event.currentTarget.contains(event.relatedTarget as Node | null)) {
                onHighlight();
              }
            }}
          >
            <button type="button" className="flex min-w-0 items-center gap-1.5 text-xs" onClick={() => onSelect(chip.id)}>
              <Icon className="size-3.5 shrink-0" aria-hidden="true" />
              <span className="max-w-36 truncate">{chip.label}</span>
              {chip.stale && <span className="text-[10px]">stale</span>}
            </button>
            {(chip.mention.kind === "region" || chip.mention.kind === "stamp" || chip.mention.kind === "object" || chip.mention.kind === "location") && (
              <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label={`${chip.label} 맵에서 찾기`} onClick={() => onFind(chip.id)}>
                <Focus className="size-3.5" aria-hidden="true" />
              </Button>
            )}
            <Button type="button" size="icon" variant="ghost" className="min-h-11 min-w-11" aria-label={`${chip.label} 멘션 제거`} onClick={() => onRemove(chip.id)}>
              <Trash2 className="size-3.5" aria-hidden="true" />
            </Button>
          </div>
        );
      })}
    </div>
  );
}
