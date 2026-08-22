import { useEffect, useMemo, useState } from "react";
import { Eraser, MousePointer2, Plus, Scan, Target, Trash2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { cellsToRows, rowsToCells, selectionBounds } from "./selectionMask";
import type {
  MapLayer,
  SelectionOperation,
  SelectionRole,
  SelectionShape,
  SavedSelection,
} from "./mapProtocol";

const roles: SelectionRole[] = ["target", "reference", "protect", "anchor"];
const layers: MapLayer[] = ["terrain", "units", "buildings", "doodads", "sprites", "locations"];

export interface SelectionToolbarProps {
  activeCells: Set<string>;
  shape: SelectionShape;
  operation: SelectionOperation;
  role: SelectionRole;
  allowedLayers: MapLayer[];
  label: string;
  interactionMode: "select" | "inspect" | "pan";
  savedSelections: SavedSelection[];
  onShape(shape: SelectionShape): void;
  onOperation(operation: SelectionOperation): void;
  onRole(role: SelectionRole): void;
  onLayers(layers: MapLayer[]): void;
  onLabel(label: string): void;
  onInteractionMode(mode: "select" | "inspect" | "pan"): void;
  onCells(cells: Set<string>): void;
  onSave(): void;
  onMention(): void;
  onClear(): void;
  onLoadSelection(selection: SavedSelection): void;
  onDeleteSelection(selection: SavedSelection): void;
}

export function SelectionToolbar({
  activeCells,
  shape,
  operation,
  role,
  allowedLayers,
  label,
  interactionMode,
  savedSelections,
  onShape,
  onOperation,
  onRole,
  onLayers,
  onLabel,
  onInteractionMode,
  onCells,
  onSave,
  onMention,
  onClear,
  onLoadSelection,
  onDeleteSelection,
}: SelectionToolbarProps) {
  const [showRows, setShowRows] = useState(false);
  const [rowText, setRowText] = useState("");
  const [rowError, setRowError] = useState("");
  const bounds = useMemo(() => selectionBounds(activeCells), [activeCells]);
  const selectionWidth = bounds ? bounds.right - bounds.left : 0;
  const selectionHeight = bounds ? bounds.bottom - bounds.top : 0;
  useEffect(() => {
    if (!showRows) return;
    setRowText(
      cellsToRows(activeCells)
        .map((row) => `${row.y}:${row.spans.map(([left, right]) => `${left}-${right}`).join(",")}`)
        .join("\n"),
    );
  }, [activeCells, showRows]);

  const applyRows = () => {
    try {
      const rows = rowText
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => {
          const [yText, spansText] = line.split(":");
          if (!yText || !spansText) throw new Error("각 행은 y:left-right 형식이어야 합니다.");
          const y = Number(yText);
          const spans = spansText.split(",").map((span) => {
            const [left, right] = span.split("-").map(Number);
            if (!Number.isInteger(left) || !Number.isInteger(right) || left >= right) {
              throw new Error("span은 left < right인 정수 범위여야 합니다.");
            }
            return [left, right] as [number, number];
          });
          return { y, spans };
        });
      onCells(rowsToCells(rows));
      setRowError("");
    } catch (error) {
      setRowError(String(error));
    }
  };

  return (
    <section className="max-h-[70vh] w-full max-w-3xl overflow-y-auto rounded-xl border border-border bg-card/95 p-3 shadow-2xl backdrop-blur">
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex rounded-md border border-border p-1" aria-label="캔버스 조작 모드">
          {(["select", "inspect", "pan"] as const).map((mode) => (
            <Button key={mode} type="button" size="sm" variant={interactionMode === mode ? "secondary" : "ghost"} aria-pressed={interactionMode === mode} onClick={() => onInteractionMode(mode)}>
              {mode === "select" ? <Scan className="size-4" aria-hidden="true" /> : <MousePointer2 className="size-4" aria-hidden="true" />}
              {mode === "select" ? "영역" : mode === "inspect" ? "개체" : "이동"}
            </Button>
          ))}
        </div>
        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          <span>Shape</span>
          <Select value={shape} onValueChange={(value) => onShape(value as SelectionShape)}>
            <SelectTrigger size="sm" className="w-32" aria-label="Shape">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="rectangle">rectangle</SelectItem>
              <SelectItem value="free">free mask</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="flex items-center gap-2 text-[11px] text-muted-foreground">
          <span>Operation</span>
          <Select
            value={operation}
            onValueChange={(value) => onOperation(value as SelectionOperation)}
          >
            <SelectTrigger size="sm" className="w-32" aria-label="Operation">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="replace">replace</SelectItem>
              <SelectItem value="add">add</SelectItem>
              <SelectItem value="subtract">subtract</SelectItem>
              <SelectItem value="invert">invert</SelectItem>
              <SelectItem value="clear">clear</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <span
          className="ml-auto font-mono text-xs tabular-nums text-amber-300"
          aria-label={`선택 크기: 가로 ${selectionWidth}셀, 세로 ${selectionHeight}셀, 총 ${activeCells.size}셀`}
        >
          {selectionWidth.toLocaleString()} × {selectionHeight.toLocaleString()} = {activeCells.size.toLocaleString()} 셀
        </span>
      </div>

      {activeCells.size > 0 && (
        <>
          <div className="mt-3 grid gap-3 md:grid-cols-[12rem_1fr]">
            <label className="text-[11px] text-muted-foreground">
              영역 이름
              <Input value={label} onChange={(event) => onLabel(event.target.value)} className="mt-1" />
            </label>
            <fieldset>
              <legend className="text-[11px] text-muted-foreground">Role</legend>
              <div className="mt-1 flex flex-wrap gap-1">
                {roles.map((value) => (
                  <Button key={value} type="button" size="sm" variant={role === value ? "secondary" : "outline"} className={cn(role === value && value === "protect" && "border-rose-400 text-rose-300")} onClick={() => onRole(value)}>
                    {value}
                  </Button>
                ))}
              </div>
            </fieldset>
          </div>
          <fieldset className="mt-3">
            <legend className="text-[11px] text-muted-foreground">스탬프 및 권한 레이어</legend>
            <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1">
              {layers.map((mapLayer) => (
                <div key={mapLayer} className="flex min-h-11 items-center gap-2 text-xs">
                  <Checkbox
                    id={`selection-layer-${mapLayer}`}
                    checked={allowedLayers.includes(mapLayer)}
                    onCheckedChange={(checked) =>
                      onLayers(
                        checked === true
                          ? [...allowedLayers, mapLayer]
                          : allowedLayers.filter((value) => value !== mapLayer),
                      )
                    }
                  />
                  <label htmlFor={`selection-layer-${mapLayer}`}>{mapLayer}</label>
                </div>
              ))}
            </div>
          </fieldset>
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" onClick={onSave}><Target className="size-4" aria-hidden="true" />영역 생성</Button>
            <Button type="button" size="sm" variant="secondary" onClick={onMention}><Plus className="size-4" aria-hidden="true" />프롬프트에 추가</Button>
            <Button type="button" size="sm" variant="outline" onClick={() => setShowRows((value) => !value)}>좌표/row-span 편집</Button>
            <Button type="button" size="sm" variant="ghost" onClick={onClear}><Eraser className="size-4" aria-hidden="true" />해제</Button>
          </div>
          {showRows && (
            <div className="mt-3 rounded-lg border border-border bg-background/70 p-2">
              <label className="text-[11px] text-muted-foreground">
                Canonical row spans · y:left-right,left-right
                <Textarea value={rowText} onChange={(event) => setRowText(event.target.value)} className="mt-1 min-h-24 font-mono text-xs" />
              </label>
              {rowError && <p role="alert" className="mt-1 text-xs text-destructive">{rowError}</p>}
              <Button type="button" size="sm" variant="outline" className="mt-2" onClick={applyRows}>row-span 적용</Button>
            </div>
          )}
        </>
      )}
      {activeCells.size === 0 && (
        <div className="mt-2 flex items-center justify-between text-xs text-muted-foreground">
          <span>Drag로 영역을 선택합니다. Ctrl=free mask, Shift=add, Esc=clear.</span>
          <Button type="button" size="sm" variant="ghost" onClick={onClear}><Trash2 className="size-4" aria-hidden="true" />초기화</Button>
        </div>
      )}
      {savedSelections.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1 border-t border-border pt-2">
          {savedSelections.map((selection) => (
            <div key={selection.id} className="flex min-h-11 items-center rounded-md border border-border bg-background/60 pl-1 text-[11px]">
              <Button type="button" size="sm" variant="ghost" className="max-w-40 min-w-0 justify-start truncate" onClick={() => onLoadSelection(selection)}>
                {selection.role}:{selection.label} · {selection.selectedCells}
              </Button>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="min-h-11 min-w-11"
                aria-label={`${selection.label} 선택 타겟 삭제`}
                onClick={() => onDeleteSelection(selection)}
              >
                <Trash2 className="size-3.5" aria-hidden="true" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
