import { useId } from "react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  FORCE_LAYOUT_OPTIONS,
  forceLayoutAvailable,
  forceMembers,
  layoutSlots,
  type ForceForm,
  type ForceLayout,
  type PlayerSlotRow,
} from "@/lib/mapSlots";

export interface ForcesEditorProps {
  /** The four CHK forces in order. */
  readonly forces: readonly ForceForm[];
  /** All 12 slots; only P1..P8 count as members or take part in quick layouts. */
  readonly players: readonly PlayerSlotRow[];
  readonly disabled?: boolean;
  readonly onForceChange: (index: number, patch: Partial<ForceForm>) => void;
  readonly onLayout: (layout: ForceLayout) => void;
}

const FORCE_FLAGS = [
  ["allied", "동맹"],
  ["alliedVictory", "동맹 승리"],
  ["sharedVision", "시야 공유"],
  ["randomStart", "시작 위치 무작위"],
] as const;

/** Four force cards plus the quick-layout presets, shared by the wizard and the properties dialog. */
export function ForcesEditor({ forces, players, disabled = false, onForceChange, onLayout }: ForcesEditorProps) {
  const idPrefix = useId();
  const layoutCount = layoutSlots(players).length;
  return (
    <div className="grid gap-4">
      <div className="grid gap-2">
        <div className="flex flex-wrap gap-2" role="group" aria-label="포스 빠른 구성">
          {FORCE_LAYOUT_OPTIONS.map((entry) => (
            <Button
              key={entry.value}
              type="button"
              variant="outline"
              size="sm"
              disabled={disabled || !forceLayoutAvailable(entry.value, layoutCount)}
              title={entry.hint}
              onClick={() => onLayout(entry.value)}
            >
              {entry.label}
            </Button>
          ))}
        </div>
        <p className="break-keep text-xs leading-5 text-muted-foreground">
          각 플레이어의 포스는 플레이어 표에서 바꿉니다. 빠른 구성은 사용 안 함·닫힘이 아닌 P1~P8의 포스를 한 번에 다시 배정합니다.
        </p>
      </div>
      <div className="grid gap-3">
        {forces.map((force, index) => {
          const members = forceMembers({ players, forces }, index);
          return (
            <div key={index} className="grid gap-2 rounded-lg border border-border p-3">
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-force-${index}`} className="text-xs font-medium text-muted-foreground">포스 {index + 1} 이름</label>
                <Input
                  id={`${idPrefix}-force-${index}`}
                  value={force.name}
                  disabled={disabled}
                  maxLength={64}
                  onChange={(event) => onForceChange(index, { name: event.target.value })}
                />
              </div>
              <p className="text-xs text-muted-foreground" aria-label={`포스 ${index + 1} 구성원`}>
                {members.length === 0
                  ? "구성원 없음"
                  : `${members.map((member) => `P${member + 1}`).join(", ")} · ${members.length}명`}
              </p>
              <div className="flex flex-wrap gap-x-5 gap-y-2 text-sm">
                {FORCE_FLAGS.map(([key, label]) => (
                  <label key={key} className="flex items-center gap-2">
                    <Checkbox
                      checked={force[key]}
                      disabled={disabled}
                      aria-label={`포스 ${index + 1} ${label}`}
                      onCheckedChange={(checked) => onForceChange(index, { [key]: checked === true })}
                    />
                    {label}
                  </label>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
