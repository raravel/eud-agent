import { useTranslation } from "react-i18next";
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
  ["allied", "mapProperties.forces.allied"],
  ["alliedVictory", "mapProperties.forces.alliedVictory"],
  ["sharedVision", "mapProperties.forces.sharedVision"],
  ["randomStart", "mapProperties.forces.randomStart"],
] as const;

/** Four force cards plus the quick-layout presets, shared by the wizard and the properties dialog. */
export function ForcesEditor({ forces, players, disabled = false, onForceChange, onLayout }: ForcesEditorProps) {
  const { t } = useTranslation();
  const idPrefix = useId();
  const layoutCount = layoutSlots(players).length;
  return (
    <div className="grid gap-4">
      <div className="grid gap-2">
        <div className="flex flex-wrap gap-2" role="group" aria-label={t("mapProperties.forces.quickLayout")}>
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
          {t("mapProperties.forces.layoutHint")}
        </p>
      </div>
      <div className="grid gap-3">
        {forces.map((force, index) => {
          const members = forceMembers({ players, forces }, index);
          return (
            <div key={index} className="grid gap-2 rounded-lg border border-border p-3">
              <div className="grid gap-1.5">
                <label htmlFor={`${idPrefix}-force-${index}`} className="text-xs font-medium text-muted-foreground">{t("mapProperties.forces.nameLabel", { n: index + 1 })}</label>
                <Input
                  id={`${idPrefix}-force-${index}`}
                  value={force.name}
                  disabled={disabled}
                  maxLength={64}
                  onChange={(event) => onForceChange(index, { name: event.target.value })}
                />
              </div>
              <p className="text-xs text-muted-foreground" aria-label={t("mapProperties.forces.membersAria", { n: index + 1 })}>
                {members.length === 0
                  ? t("mapProperties.forces.noMembers")
                  : t("mapProperties.forces.members", { count: members.length, list: members.map((member) => `P${member + 1}`).join(", ") })}
              </p>
              <div className="flex flex-wrap gap-x-5 gap-y-2 text-sm">
                {FORCE_FLAGS.map(([key, label]) => (
                  <label key={key} className="flex items-center gap-2">
                    <Checkbox
                      checked={force[key]}
                      disabled={disabled}
                      aria-label={t("mapProperties.forces.flagAria", { n: index + 1, label: t(label) })}
                      onCheckedChange={(checked) => onForceChange(index, { [key]: checked === true })}
                    />
                    {t(label)}
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
