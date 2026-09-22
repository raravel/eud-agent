// Shared 12-slot / 4-force player model used by the blank-map wizard and the Map window's
// properties dialog. Pure data and helpers only; wire encoding lives in the callers.

export type SlotType = "human" | "computer" | "rescuable" | "neutral" | "inactive" | "closed";
export type Race = "zerg" | "terran" | "protoss" | "userSelectable" | "random" | "neutral" | "inactive";
/** CHK SIDE value 3 exists in old maps; it is displayed but never offered as a choice. */
export type DisplayRace = Race | "independent";

/** Every CHK player slot (OWNR/SIDE). */
export const TOTAL_SLOTS = 12;
/** Slots that can play, hold a start location and belong to a force (FORC has 8 entries). */
export const MAX_PLAYERS = 8;
export const MAX_FORCES = 4;

export const SLOT_TYPE_OPTIONS: readonly { value: SlotType; label: string }[] = [
  { value: "human", label: "사람" },
  { value: "computer", label: "컴퓨터" },
  { value: "rescuable", label: "구출 가능" },
  { value: "neutral", label: "중립" },
  { value: "inactive", label: "사용 안 함" },
  { value: "closed", label: "닫힘" },
];

export const RACE_OPTIONS: readonly { value: Race; label: string }[] = [
  { value: "userSelectable", label: "선택 가능" },
  { value: "terran", label: "테란" },
  { value: "zerg", label: "저그" },
  { value: "protoss", label: "프로토스" },
  { value: "random", label: "랜덤" },
  { value: "neutral", label: "중립" },
  { value: "inactive", label: "사용 안 함" },
];

export const INDEPENDENT_RACE_LABEL = "독립";

export type ForceLayout = "single" | "individual" | "teams";

/** Quick presets that rewrite every playable slot's force; the result stays freely editable. */
export const FORCE_LAYOUT_OPTIONS: readonly { value: ForceLayout; label: string; hint: string }[] = [
  { value: "single", label: "모두 한 포스", hint: "전원이 포스 1에 속합니다" },
  { value: "individual", label: "각자 개별 포스", hint: "플레이어마다 포스 하나 (최대 4명)" },
  { value: "teams", label: "2팀으로 나누기", hint: "앞 절반은 포스 1, 뒤 절반은 포스 2" },
];

export interface PlayerForm {
  readonly type: SlotType;
  readonly race: Race;
  /** 0-based index into the four forces; meaningful for P1..P8 only, always 0 for P9..P12. */
  readonly force: number;
}

/** A slot row as the editors see it: the race may be the read-only legacy value. */
export interface PlayerSlotRow {
  readonly type: SlotType;
  readonly race: DisplayRace;
  readonly force: number;
}

export interface PlayerPatch {
  readonly type?: SlotType;
  readonly race?: Race;
  readonly force?: number;
}

export interface ForceForm {
  readonly name: string;
  readonly allied: boolean;
  readonly alliedVictory: boolean;
  readonly sharedVision: boolean;
  readonly randomStart: boolean;
}

/** The part of a form both editors operate on: always 12 players and 4 forces. */
export interface SlotsForm {
  readonly players: readonly PlayerSlotRow[];
  readonly forces: readonly ForceForm[];
}

export function defaultForce(index: number): ForceForm {
  return {
    name: `포스 ${index + 1}`,
    allied: true,
    alliedVictory: true,
    sharedVision: false,
    randomStart: false,
  };
}

export function defaultForces(): ForceForm[] {
  return Array.from({ length: MAX_FORCES }, (_, index) => defaultForce(index));
}

/** P1..P8 open to humans, P9..P11 unused, P12 neutral: the SCMDraft 2 new-map layout. */
export function defaultPlayer(slot: number): PlayerForm {
  if (slot < MAX_PLAYERS) return { type: "human", race: "userSelectable", force: 0 };
  if (slot === TOTAL_SLOTS - 1) return { type: "neutral", race: "neutral", force: 0 };
  return { type: "inactive", race: "inactive", force: 0 };
}

export function defaultPlayers(): PlayerForm[] {
  return Array.from({ length: TOTAL_SLOTS }, (_, slot) => defaultPlayer(slot));
}

/** Slot types that actually play and therefore receive a start location. */
export function slotNeedsStart(type: SlotType): boolean {
  return type === "human" || type === "computer" || type === "rescuable";
}

/** Slots the quick force layouts distribute: P1..P8 that are neither unused nor closed. */
export function layoutSlots(players: readonly PlayerSlotRow[]): number[] {
  return players.flatMap((player, index) =>
    index < MAX_PLAYERS && player.type !== "inactive" && player.type !== "closed" ? [index] : [],
  );
}

export function forceLayoutAvailable(layout: ForceLayout, playerCount: number): boolean {
  if (layout === "individual") return playerCount <= MAX_FORCES;
  if (layout === "teams") return playerCount >= 2;
  return true;
}

/** Force index per layout position (0-based) for a layout. */
export function forceAssignment(layout: ForceLayout, playerCount: number): number[] {
  const count = Math.max(0, Math.min(playerCount, MAX_PLAYERS));
  if (layout === "individual" && count <= MAX_FORCES) {
    return Array.from({ length: count }, (_, index) => index);
  }
  if (layout === "teams" && count >= 2) {
    const firstTeam = Math.ceil(count / 2);
    return Array.from({ length: count }, (_, index) => (index < firstTeam ? 0 : 1));
  }
  return Array.from({ length: count }, () => 0);
}

/** Assign one P1..P8 slot to a force. Out-of-range slots or forces leave the form unchanged. */
export function withPlayerForce<T extends SlotsForm>(form: T, playerIndex: number, force: number): T {
  if (playerIndex < 0 || playerIndex >= MAX_PLAYERS) return form;
  if (!Number.isInteger(force) || force < 0 || force >= form.forces.length) return form;
  return {
    ...form,
    players: form.players.map((player, index) => (index === playerIndex ? { ...player, force } : player)),
  };
}

/** Apply a type/race/force patch to one slot; the force part follows `withPlayerForce`. */
export function withPlayerPatch<T extends SlotsForm>(form: T, playerIndex: number, patch: PlayerPatch): T {
  const { force, ...rest } = patch;
  const patched = force === undefined ? form : withPlayerForce(form, playerIndex, force);
  if (Object.keys(rest).length === 0) return patched;
  return {
    ...patched,
    players: patched.players.map((player, index) => (index === playerIndex ? { ...player, ...rest } : player)),
  };
}

export function withForcePatch<T extends SlotsForm>(form: T, forceIndex: number, patch: Partial<ForceForm>): T {
  if (forceIndex < 0 || forceIndex >= form.forces.length) return form;
  return {
    ...form,
    forces: form.forces.map((force, index) => (index === forceIndex ? { ...force, ...patch } : force)),
  };
}

/** Apply a quick preset over the layout slots; every other slot returns to force 1. */
export function applyForceLayout<T extends SlotsForm>(form: T, layout: ForceLayout): T {
  const slots = layoutSlots(form.players);
  if (!forceLayoutAvailable(layout, slots.length)) return form;
  const assignment = forceAssignment(layout, slots.length);
  return {
    ...form,
    players: form.players.map((player, index) => {
      if (index >= MAX_PLAYERS) return player;
      const position = slots.indexOf(index);
      return { ...player, force: position === -1 ? 0 : (assignment[position] ?? 0) };
    }),
  };
}

/** 0-based P1..P8 slot indexes currently assigned to `force`. */
export function forceMembers(form: SlotsForm, force: number): number[] {
  return form.players.flatMap((player, index) => (index < MAX_PLAYERS && player.force === force ? [index] : []));
}

/** Display label for a force: its trimmed name, or the positional fallback when blank. */
export function forceLabel(force: ForceForm, index: number): string {
  const name = force.name.trim();
  return name === "" ? `포스 ${index + 1}` : name;
}
