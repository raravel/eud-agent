import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  forceLabel,
  INDEPENDENT_RACE_LABEL,
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
  return (
    <table className="w-full text-sm" aria-label="플레이어 슬롯">
      <thead>
        <tr className="text-left text-xs text-muted-foreground">
          <th scope="col" className="pb-2 pr-3 font-medium">슬롯</th>
          <th scope="col" className="pb-2 pr-3 font-medium">타입</th>
          <th scope="col" className="pb-2 pr-3 font-medium">종족</th>
          <th scope="col" className="pb-2 font-medium">포스</th>
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
                <SelectTrigger aria-label={`P${index + 1} 타입`} className="w-full">
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
                <SelectTrigger aria-label={`P${index + 1} 종족`} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {RACE_OPTIONS.map((entry) => (
                    <SelectItem key={entry.value} value={entry.value}>{entry.label}</SelectItem>
                  ))}
                  {/* Legacy SIDE value: shown so the current value stays visible, never re-chosen. */}
                  {player.race === "independent" && (
                    <SelectItem value="independent" disabled>{INDEPENDENT_RACE_LABEL}</SelectItem>
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
                  <SelectTrigger aria-label={`P${index + 1} 포스`} className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {forces.map((force, forceIndex) => (
                      <SelectItem key={forceIndex} value={String(forceIndex)}>{forceLabel(force, forceIndex)}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <span className="block px-3 text-muted-foreground" aria-label={`P${index + 1} 포스 없음`}>—</span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
