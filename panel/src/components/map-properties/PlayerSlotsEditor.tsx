import { useTranslation } from "react-i18next";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  forceLabel,
  MAX_PLAYERS,
  RACE_OPTIONS,
  SLOT_TYPE_OPTIONS,
  type ForceForm,
  type PlayerPatch,
  type PlayerSlotRow,
  type Race,
  type SlotType,
} from "@/lib/mapSlots";

export interface PlayerSlotsEditorProps {
  /** All 12 CHK slots in order. */
  readonly players: readonly PlayerSlotRow[];
  /** The four forces, used for the P1..P8 force column labels. */
  readonly forces: readonly ForceForm[];
  readonly disabled?: boolean;
  readonly onPlayerChange: (index: number, patch: PlayerPatch) => void;
}

/** 12-row slot table shared by the blank-map wizard and the Map window's properties dialog. */
export function PlayerSlotsEditor({ players, forces, disabled = false, onPlayerChange }: PlayerSlotsEditorProps) {
  const { t } = useTranslation();
  return (
    <table className="w-full text-sm" aria-label={t("mapProperties.players.table")}>
      <thead>
        <tr className="text-left text-xs text-muted-foreground">
          <th scope="col" className="pb-2 pr-3 font-medium">{t("mapProperties.players.slot")}</th>
          <th scope="col" className="pb-2 pr-3 font-medium">{t("mapProperties.players.type")}</th>
          <th scope="col" className="pb-2 pr-3 font-medium">{t("mapProperties.players.race")}</th>
          <th scope="col" className="pb-2 font-medium">{t("mapProperties.players.force")}</th>
        </tr>
      </thead>
      <tbody>
        {players.map((player, index) => (
          <tr key={index} className="border-t border-border">
            <th scope="row" className="py-2 pr-3 font-medium">P{index + 1}</th>
            <td className="py-2 pr-3">
              <Select
                value={player.type}
                disabled={disabled}
                onValueChange={(value) => onPlayerChange(index, { type: value as SlotType })}
              >
                <SelectTrigger aria-label={t("mapProperties.players.typeAria", { n: index + 1 })} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SLOT_TYPE_OPTIONS.map((entry) => (
                    <SelectItem key={entry.value} value={entry.value}>{entry.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </td>
            <td className="py-2 pr-3">
              <Select
                value={player.race}
                disabled={disabled}
                onValueChange={(value) => {
                  if (value !== "independent") onPlayerChange(index, { race: value as Race });
                }}
              >
                <SelectTrigger aria-label={t("mapProperties.players.raceAria", { n: index + 1 })} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {RACE_OPTIONS.map((entry) => (
                    <SelectItem key={entry.value} value={entry.value}>{entry.label}</SelectItem>
                  ))}
                  {/* Legacy SIDE value: shown so the current value stays visible, never re-chosen. */}
                  {player.race === "independent" && (
                    <SelectItem value="independent" disabled>{t("messages.mapSlots.independent")}</SelectItem>
                  )}
                </SelectContent>
              </Select>
            </td>
            <td className="py-2">
              {index < MAX_PLAYERS ? (
                <Select
                  value={String(player.force)}
                  disabled={disabled}
                  onValueChange={(value) => onPlayerChange(index, { force: Number(value) })}
                >
                  <SelectTrigger aria-label={t("mapProperties.players.forceAria", { n: index + 1 })} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {forces.map((force, forceIndex) => (
                      <SelectItem key={forceIndex} value={String(forceIndex)}>{forceLabel(force, forceIndex)}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <span className="block px-3 text-muted-foreground" aria-label={t("mapProperties.players.noForceAria", { n: index + 1 })}>—</span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
