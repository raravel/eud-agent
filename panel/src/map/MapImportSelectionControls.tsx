import { Eraser, LoaderCircle, Save } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type {
  MapLayer,
  SelectionOperation,
  SelectionShape,
  TileRect,
} from "./mapProtocol";

const layerLabels: Record<MapLayer, string> = {
  terrain: "지형",
  units: "유닛",
  buildings: "건물",
  doodads: "두다드",
  sprites: "스프라이트",
  locations: "로케이션",
};

const operations: Array<{
  value: Exclude<SelectionOperation, "clear">;
  label: string;
}> = [
  { value: "replace", label: "교체" },
  { value: "add", label: "추가" },
  { value: "subtract", label: "빼기" },
  { value: "invert", label: "반전" },
];

export interface MapImportSelectionControlsProps {
  shape: SelectionShape;
  operation: SelectionOperation;
  layers: MapLayer[];
  label: string;
  selectedCells: number;
  bounds: TileRect | null;
  saving: boolean;
  disabled: boolean;
  onShape(shape: SelectionShape): void;
  onOperation(operation: SelectionOperation): void;
  onLayers(layers: MapLayer[]): void;
  onLabel(label: string): void;
  onClear(): void;
  onSave(): void;
}

export function MapImportSelectionControls({
  shape,
  operation,
  layers,
  label,
  selectedCells,
  bounds,
  saving,
  disabled,
  onShape,
  onOperation,
  onLayers,
  onLabel,
  onClear,
  onSave,
}: MapImportSelectionControlsProps) {
  return (
    <aside className="flex min-h-0 flex-col gap-4 overflow-y-auto border-r border-border bg-card/70 p-4">
      <section>
        <h2 className="text-sm font-semibold">선택 방식</h2>
        <div className="mt-2 grid grid-cols-2 gap-2">
          <Button
            type="button"
            variant={shape === "rectangle" ? "secondary" : "outline"}
            onClick={() => onShape("rectangle")}
          >
            사각형
          </Button>
          <Button
            type="button"
            variant={shape === "free" ? "secondary" : "outline"}
            onClick={() => onShape("free")}
          >
            자유 마스크
          </Button>
        </div>
        <div className="mt-2 grid grid-cols-2 gap-2">
          {operations.map((item) => (
            <Button
              key={item.value}
              type="button"
              variant={operation === item.value ? "secondary" : "outline"}
              onClick={() => onOperation(item.value)}
            >
              {item.label}
            </Button>
          ))}
          <Button type="button" variant="outline" onClick={onClear}>
            <Eraser className="size-4" aria-hidden="true" />
            지우기
          </Button>
        </div>
      </section>

      <section>
        <h2 className="text-sm font-semibold">복사 레이어</h2>
        <div className="mt-2 grid grid-cols-2 gap-2">
          {(Object.keys(layerLabels) as MapLayer[]).map((layer) => {
            const selected = layers.includes(layer);
            return (
              <Button
                key={layer}
                type="button"
                variant={selected ? "secondary" : "outline"}
                aria-pressed={selected}
                onClick={() =>
                  onLayers(
                    selected
                      ? layers.filter((value) => value !== layer)
                      : [...layers, layer],
                  )
                }
              >
                {layerLabels[layer]}
              </Button>
            );
          })}
        </div>
        <p className="mt-2 text-xs text-muted-foreground">
          선택 없음은 exact stamp 계약에 따라 여섯 레이어 전체로 해석됩니다.
        </p>
      </section>

      <section className="space-y-2">
        <label htmlFor="map-import-label" className="text-sm font-semibold">
          팔레트 이름
        </label>
        <Input
          id="map-import-label"
          value={label}
          maxLength={80}
          onChange={(event) => onLabel(event.target.value)}
          placeholder="예: 언덕 입구"
        />
        <div className="flex flex-wrap gap-2 text-xs">
          <Badge variant="outline">{selectedCells.toLocaleString()}셀</Badge>
          {bounds && (
            <Badge variant="outline">
              {bounds.right - bounds.left}×{bounds.bottom - bounds.top}
            </Badge>
          )}
        </div>
      </section>

      <Button
        type="button"
        className="mt-auto min-h-11"
        disabled={disabled || saving || selectedCells === 0 || label.trim().length === 0}
        onClick={onSave}
      >
        {saving ? (
          <LoaderCircle className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <Save className="size-4" aria-hidden="true" />
        )}
        프로젝트 팔레트에 추가
      </Button>
    </aside>
  );
}
