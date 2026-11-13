import { useTranslation } from "react-i18next";
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

const layerLabelKeys: Record<MapLayer, string> = {
  terrain: "map.layers.terrain",
  units: "map.layers.units",
  buildings: "map.layers.buildings",
  doodads: "map.layers.doodads",
  sprites: "map.layers.sprites",
  locations: "map.layers.locations",
};

const operations: Array<{
  value: Exclude<SelectionOperation, "clear">;
  label: string;
}> = [
  { value: "replace", label: "mapImport.selection.replace" },
  { value: "add", label: "mapImport.selection.add" },
  { value: "subtract", label: "mapImport.selection.subtract" },
  { value: "invert", label: "mapImport.selection.invert" },
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
  const { t } = useTranslation();
  return (
    <aside className="flex min-h-0 flex-col gap-4 overflow-y-auto border-r border-border bg-card/70 p-4">
      <section>
        <h2 className="text-sm font-semibold">{t("mapImport.selection.method")}</h2>
        <div className="mt-2 grid grid-cols-2 gap-2">
          <Button
            type="button"
            variant={shape === "rectangle" ? "secondary" : "outline"}
            onClick={() => onShape("rectangle")}
          >
            {t("mapImport.selection.rectangle")}
          </Button>
          <Button
            type="button"
            variant={shape === "free" ? "secondary" : "outline"}
            onClick={() => onShape("free")}
          >
            {t("mapImport.selection.free")}
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
              {t(item.label)}
            </Button>
          ))}
          <Button type="button" variant="outline" onClick={onClear}>
            <Eraser className="size-4" aria-hidden="true" />
            {t("mapImport.selection.clear")}
          </Button>
        </div>
      </section>

      <section>
        <h2 className="text-sm font-semibold">{t("mapImport.selection.copyLayers")}</h2>
        <div className="mt-2 grid grid-cols-2 gap-2">
          {(Object.keys(layerLabelKeys) as MapLayer[]).map((layer) => {
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
                {t(layerLabelKeys[layer])}
              </Button>
            );
          })}
        </div>
        <p className="mt-2 text-xs text-muted-foreground">
          {t("mapImport.selection.noneMeansAll")}
        </p>
      </section>

      <section className="space-y-2">
        <label htmlFor="map-import-label" className="text-sm font-semibold">
          {t("mapImport.selection.paletteName")}
        </label>
        <Input
          id="map-import-label"
          value={label}
          maxLength={80}
          onChange={(event) => onLabel(event.target.value)}
          placeholder={t("mapImport.selection.placeholder")}
        />
        <div className="flex flex-wrap gap-2 text-xs">
          <Badge variant="outline">{t("map.cells", { count: selectedCells, n: selectedCells.toLocaleString() })}</Badge>
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
        {t("mapImport.selection.addToPalette")}
      </Button>
    </aside>
  );
}
