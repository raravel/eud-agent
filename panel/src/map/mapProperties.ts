import {
  defaultForce,
  MAX_FORCES,
  MAX_PLAYERS,
  TOTAL_SLOTS,
  type DisplayRace,
  type ForceForm,
  type PlayerSlotRow,
  type SlotType,
} from "@/lib/mapSlots";
import type { MapDigest, MapPropertiesRequest } from "./mapProtocol";

/** Editable scenario properties mirrored from the Map window digest. */
export interface MapPropertiesForm {
  readonly title: string;
  readonly description: string;
  /** Always 12 slots. */
  readonly players: readonly PlayerSlotRow[];
  /** Always 4 forces. */
  readonly forces: readonly ForceForm[];
}

/** OWNR controller id → slot type. 1/2/4 are legacy ids displayed as their nearest choice. */
export function slotTypeFromControllerId(controllerId: number): SlotType {
  switch (controllerId) {
    case 1:
    case 5:
      return "computer";
    case 2:
    case 6:
      return "human";
    case 3:
      return "rescuable";
    case 7:
      return "neutral";
    case 8:
      return "closed";
    default:
      return "inactive";
  }
}

/** SIDE race id → race. 3 (independent) is display-only and never offered as a choice. */
export function raceFromRaceId(raceId: number): DisplayRace {
  switch (raceId) {
    case 0:
      return "zerg";
    case 1:
      return "terran";
    case 2:
      return "protoss";
    case 3:
      return "independent";
    case 4:
      return "neutral";
    case 5:
      return "userSelectable";
    case 6:
      return "random";
    default:
      return "inactive";
  }
}

/**
 * Build the editable form from a Map window digest. Returns null when the digest predates map
 * properties (no players/forces), so the dialog can refuse instead of saving defaults.
 */
export function propertiesFormFromDigest(digest: MapDigest): MapPropertiesForm | null {
  if (digest.players === undefined || digest.forces === undefined) return null;
  const players = Array.from({ length: TOTAL_SLOTS }, (_, slot): PlayerSlotRow => {
    const entry = digest.players?.find((player) => player.player === `P${slot + 1}`) ?? digest.players?.[slot];
    if (entry === undefined) return { type: "inactive", race: "inactive", force: 0 };
    const force = slot < MAX_PLAYERS && entry.force !== undefined ? entry.force - 1 : 0;
    return {
      type: slotTypeFromControllerId(entry.controllerId),
      race: raceFromRaceId(entry.raceId),
      force: force >= 0 && force < MAX_FORCES ? force : 0,
    };
  });
  const forces = Array.from({ length: MAX_FORCES }, (_, index): ForceForm => {
    const entry = digest.forces?.find((force) => force.force === index + 1) ?? digest.forces?.[index];
    if (entry === undefined) return defaultForce(index);
    return {
      name: entry.name,
      allied: entry.flags.allies,
      alliedVictory: entry.flags.alliedVictory,
      sharedVision: entry.flags.sharedVision,
      randomStart: entry.flags.randomStartLocation,
    };
  });
  return {
    title: digest.map.title ?? "",
    description: digest.map.description ?? "",
    players,
    forces,
  };
}

export function propertiesFormEqual(a: MapPropertiesForm, b: MapPropertiesForm): boolean {
  if (a.title !== b.title || a.description !== b.description) return false;
  if (a.players.length !== b.players.length || a.forces.length !== b.forces.length) return false;
  return (
    a.players.every((player, index) => {
      const other = b.players[index]!;
      return player.type === other.type && player.race === other.race && player.force === other.force;
    }) &&
    a.forces.every((force, index) => {
      const other = b.forces[index]!;
      return (
        force.name === other.name &&
        force.allied === other.allied &&
        force.alliedVictory === other.alliedVictory &&
        force.sharedVision === other.sharedVision &&
        force.randomStart === other.randomStart
      );
    })
  );
}

export function validateMapProperties(form: MapPropertiesForm): string | null {
  if (form.title.trim() === "") return "맵 제목을 입력해 주세요.";
  if (form.forces.some((force) => force.name.trim() === "")) return "포스 이름을 모두 입력해 주세요.";
  return null;
}

/**
 * Encode the form for `map_agent_properties_save`. A read-only `independent` race is sent back as
 * itself so an untouched legacy slot produces no change.
 */
export function buildMapPropertiesRequest(form: MapPropertiesForm): MapPropertiesRequest {
  return {
    title: form.title.trim(),
    description: form.description.trim(),
    players: form.players.slice(0, TOTAL_SLOTS).map((player, slot) => ({
      type: player.type,
      race: player.race,
      ...(slot < MAX_PLAYERS ? { force: player.force } : {}),
    })),
    forces: form.forces.slice(0, MAX_FORCES).map((force) => ({
      name: force.name.trim(),
      allied: force.allied,
      alliedVictory: force.alliedVictory,
      sharedVision: force.sharedVision,
      randomStart: force.randomStart,
    })),
  };
}
