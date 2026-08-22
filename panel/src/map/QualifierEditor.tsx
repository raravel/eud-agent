import { SlidersHorizontal } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  MentionChip,
  MentionQualifiers,
  SavedSelection,
  TileRect,
} from "./mapProtocol";

export interface QualifierEditorProps {
  chip?: MentionChip;
  onChange(qualifiers: MentionQualifiers): void;
  selections?: SavedSelection[];
  mapWidth?: number;
  mapHeight?: number;
}

export function QualifierEditor({
  chip,
  onChange,
  selections = [],
  mapWidth = 65_535,
  mapHeight = 65_535,
}: QualifierEditorProps) {
  if (!chip) {
    return (
      <div className="rounded-lg border border-dashed border-border p-3 text-xs text-muted-foreground">
        멘션 칩을 선택하면 owner, count, state를 편집할 수 있습니다.
      </div>
    );
  }
  if (chip.mention.kind !== "palette") {
    return (
      <div className="rounded-lg border border-border bg-background/40 p-3 text-xs">
        <div className="font-medium">{chip.label}</div>
        <p className="mt-1 text-muted-foreground">
          {chip.mention.kind === "region"
            ? "영역 권한은 저장된 선택 스냅샷에서 검증됩니다."
            : chip.mention.kind === "stamp"
              ? "영역 스탬프는 배치 시점의 현재 후보 내용과 선택 레이어를 정확히 복사합니다."
              : chip.mention.kind === "object"
                ? "캔버스 instance 멘션은 후보 revision과 fingerprint에 고정됩니다."
                : "기존 로케이션 ID는 재정렬되지 않습니다."}
        </p>
      </div>
    );
  }

  const qualifiers = chip.mention.qualifiers;
  const layer = chip.mention.entry.layer;
  const setNumber = (key: keyof MentionQualifiers, value: string) => {
    const next = { ...qualifiers };
    if (value.trim() === "") delete next[key];
    else Object.assign(next, { [key]: Number(value) });
    onChange(next);
  };
  const numericFields: Array<{ key: keyof MentionQualifiers; label: string; min: number; max: number }> = [
    { key: "owner", label: "Owner (0–11)", min: 0, max: 11 },
    { key: "count", label: "Count", min: 1, max: 999 },
    { key: "facing", label: "Facing (0–255)", min: 0, max: 255 },
    { key: "hpPercent", label: "HP %", min: 0, max: 100 },
    { key: "shieldPercent", label: "Shield %", min: 0, max: 100 },
    { key: "energyPercent", label: "Energy %", min: 0, max: 100 },
    { key: "resourceAmount", label: "Resource", min: 0, max: 65535 },
  ];
  const visibleFields =
    layer === "units" || layer === "buildings"
      ? numericFields
      : layer === "sprites" || layer === "doodads"
        ? numericFields.slice(0, 3)
        : [];
  const setLocationBounds = (key: keyof TileRect, value: string) => {
    const bounds = qualifiers.locationBounds ?? { left: 0, top: 0, right: 1, bottom: 1 };
    onChange({
      ...qualifiers,
      locationBounds: { ...bounds, [key]: Number(value) },
      locationSelection: undefined,
    });
  };
  const setLocationSource = (value: string) => {
    const next = { ...qualifiers };
    delete next.locationSelection;
    delete next.locationBounds;
    if (value === "__direct") {
      next.locationBounds = { left: 0, top: 0, right: 1, bottom: 1 };
    } else {
      const selection = selections.find((item) => item.id === value);
      if (selection) {
        next.locationSelection = {
          selectionId: selection.id,
          snapshotHash: selection.snapshotHash,
          sourceRevision: selection.sourceRevision,
        };
      }
    }
    onChange(next);
  };

  return (
    <section className="rounded-lg border border-border bg-background/40 p-3">
      <h3 className="flex items-center gap-2 text-xs font-semibold">
        <SlidersHorizontal className="size-4" aria-hidden="true" />
        {chip.label} 설정
      </h3>
      {visibleFields.length > 0 && (
        <div className="mt-3 grid grid-cols-2 gap-2">
          {visibleFields.map((field) => (
            <label key={field.key} className="space-y-1 text-[11px] text-muted-foreground">
              <span>{field.label}</span>
              <Input
                type="number"
                min={field.min}
                max={field.max}
                value={qualifiers[field.key]?.toString() ?? ""}
                onChange={(event) => setNumber(field.key, event.target.value)}
              />
            </label>
          ))}
        </div>
      )}
      {(layer === "units" || layer === "buildings") && (
        <div className="mt-3 flex min-h-11 items-center gap-2 text-xs">
          <Checkbox
            id="qualifier-invincible"
            checked={qualifiers.invincible ?? false}
            onCheckedChange={(checked) =>
              onChange({ ...qualifiers, invincible: checked === true })
            }
          />
          <label htmlFor="qualifier-invincible">Invincible</label>
        </div>
      )}
      {chip.mention.entry.kind === "newLocation" && (
        <div className="mt-3 space-y-2 text-[11px] text-muted-foreground">
          <label className="block space-y-1">
            <span>로케이션 이름</span>
            <Input
              value={qualifiers.locationName ?? ""}
              onChange={(event) => onChange({ ...qualifiers, locationName: event.target.value })}
            />
          </label>
          <div className="block space-y-1">
            <span>Bounds source</span>
            <Select
              value={
                qualifiers.locationSelection?.selectionId ??
                (qualifiers.locationBounds ? "__direct" : "")
              }
              onValueChange={setLocationSource}
            >
              <SelectTrigger className="w-full" aria-label="Bounds source">
                <SelectValue placeholder="범위를 선택하세요" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="__direct">직접 tile bounds</SelectItem>
                {selections.map((selection) => (
                  <SelectItem key={selection.id} value={selection.id}>
                    {selection.label} · {selection.selectedCells} cells
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          {qualifiers.locationBounds && (
            <div className="grid grid-cols-2 gap-2">
              {(
                [
                  ["left", "Tile left", 0, Math.max(0, mapWidth - 1)],
                  ["top", "Tile top", 0, Math.max(0, mapHeight - 1)],
                  ["right", "Tile right exclusive", 1, mapWidth],
                  ["bottom", "Tile bottom exclusive", 1, mapHeight],
                ] as const
              ).map(([key, label, min, max]) => (
                <label key={key} className="space-y-1">
                  <span>{label}</span>
                  <Input
                    type="number"
                    min={min}
                    max={max}
                    value={qualifiers.locationBounds?.[key] ?? ""}
                    onChange={(event) => setLocationBounds(key, event.target.value)}
                  />
                </label>
              ))}
            </div>
          )}
        </div>
      )}
      {visibleFields.length === 0 && chip.mention.entry.kind !== "newLocation" && (
        <p className="mt-2 text-[11px] text-muted-foreground">
          이 타입은 추가 qualifier 없이 정확한 catalog fingerprint를 유지합니다.
        </p>
      )}
    </section>
  );
}
