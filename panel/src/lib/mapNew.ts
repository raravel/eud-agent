import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type { InvokeFn } from "@/lib/ipc";
import {
  defaultForces,
  defaultPlayers,
  forceLabel,
  MAX_FORCES,
  MAX_PLAYERS,
  slotNeedsStart,
  TOTAL_SLOTS,
  type ForceForm,
  type PlayerForm,
  type Race,
  type SlotType,
} from "@/lib/mapSlots";
import { isServerMessage, type SetupMessage } from "@/lib/protocol";

export {
  applyForceLayout,
  defaultForce,
  defaultForces,
  defaultPlayer,
  defaultPlayers,
  FORCE_LAYOUT_OPTIONS,
  forceAssignment,
  forceLabel,
  forceLayoutAvailable,
  forceMembers,
  layoutSlots,
  MAX_FORCES,
  MAX_PLAYERS,
  RACE_OPTIONS,
  SLOT_TYPE_OPTIONS,
  slotNeedsStart,
  TOTAL_SLOTS,
  withForcePatch,
  withPlayerForce,
  withPlayerPatch,
} from "@/lib/mapSlots";
export type { ForceForm, ForceLayout, PlayerForm, PlayerPatch, Race, SlotType } from "@/lib/mapSlots";

// ---- wire types (mirror src-tauri/src/blank_project.rs + crates/isom MapNewSpec) ----

export type MapVersion = "remastered" | "broodWar";

export interface MapNewStart {
  readonly x: number;
  readonly y: number;
}

export interface MapNewPlayer {
  readonly slot: number;
  readonly type: SlotType;
  readonly race: Race;
  /** 0..3; present only for slots 0..7 (FORC has eight entries). */
  readonly force?: number;
  /** Only slots 0..7 may hold a start location. */
  readonly start?: MapNewStart;
}

export interface MapNewForce {
  readonly name: string;
  readonly allied: boolean;
  readonly alliedVictory: boolean;
  readonly sharedVision: boolean;
  readonly randomStart: boolean;
}

export interface MapNewSpec {
  readonly version: MapVersion;
  readonly tileset: number;
  readonly width: number;
  readonly height: number;
  readonly terrainType: number;
  readonly title: string;
  readonly description: string;
  readonly players: readonly MapNewPlayer[];
  readonly forces: readonly MapNewForce[];
}

export interface TilesetOption {
  readonly id: number;
  readonly key: string;
  readonly label: string;
}

export interface StarcraftAvailability {
  readonly available: boolean;
  readonly path: string;
  readonly reason?: string;
}

export interface MapNewOptions {
  readonly starcraft: StarcraftAvailability;
  readonly tilesets: readonly TilesetOption[];
  readonly sizePresets: readonly number[];
  readonly sizeMin: number;
  readonly sizeMax: number;
}

export interface BrushOption {
  readonly id: number;
  readonly name: string;
  readonly graphicsValid: boolean;
}

export interface BlankProjectPreview {
  readonly mapPath: string;
  readonly outputMap: string;
  readonly width: number;
  readonly height: number;
  readonly tileset: string;
  readonly players: number;
  readonly startLocations: number;
  readonly previewPng: string;
}

export interface BlankProjectRequest {
  readonly destination: string;
  readonly name: string;
  readonly spec: MapNewSpec;
}

export interface BlankProjectResult {
  readonly setup: SetupMessage;
  readonly preview: BlankProjectPreview | null;
}

// ---- constants ----

export const MAP_SIZE_MIN = 64;
export const MAP_SIZE_MAX = 256;
/** Tiles kept clear between the map edge and an auto-placed start location. */
export const START_MARGIN_TILES = 4;
/** Start location footprint is 4x3 tiles; the CHK stores its centre in pixels. */
const START_HALF_WIDTH_PX = 64;
const START_HALF_HEIGHT_PX = 48;
const TILE_PX = 32;
/** Auto-placed start locations are packed four per row in the top-left corner. */
const START_CLUSTER_COLUMNS = 4;

export const VERSION_OPTIONS: readonly { value: MapVersion; label: string; hint: string }[] = [
  { value: "remastered", label: "리마스터 (.scx)", hint: "StarCraft: Remastered 기본 형식" },
  { value: "broodWar", label: "브루드 워 (.scx)", hint: "VER 205 형식 · 문자열 테이블 텍스트는 SCMDraft 2가 읽도록 CP949로 저장" },
];

// ---- pure helpers ----

/**
 * Start-location preset in pixels: every start location is packed into the top-left corner,
 * inside the margin, as a left-to-right grid of 4x3-tile cells (four per row) so P1..P8 read
 * in order and nothing overlaps. The author moves them afterwards; there is no spread layout.
 */
export function startLocationPreset(width: number, height: number, count: number): MapNewStart[] {
  const total = Math.max(0, Math.min(count, MAX_PLAYERS));
  const left = START_MARGIN_TILES * TILE_PX + START_HALF_WIDTH_PX;
  const top = START_MARGIN_TILES * TILE_PX + START_HALF_HEIGHT_PX;
  const maxX = width * TILE_PX - left;
  const maxY = height * TILE_PX - top;
  return Array.from({ length: total }, (_, index) => ({
    x: Math.min(left + (index % START_CLUSTER_COLUMNS) * 2 * START_HALF_WIDTH_PX, maxX),
    y: Math.min(top + Math.floor(index / START_CLUSTER_COLUMNS) * 2 * START_HALF_HEIGHT_PX, maxY),
  }));
}

export interface WizardForm {
  readonly name: string;
  readonly title: string;
  readonly description: string;
  readonly version: MapVersion;
  readonly tileset: number;
  readonly width: number;
  readonly height: number;
  readonly terrainType: number;
  /** Always all 12 CHK slots; P1..P8 index `forces`, P9..P12 keep force 0. */
  readonly players: readonly PlayerForm[];
  /** Always the four CHK forces. */
  readonly forces: readonly ForceForm[];
  readonly autoStart: boolean;
}

export function defaultWizardForm(): WizardForm {
  return {
    name: "",
    title: "",
    description: "",
    version: "remastered",
    tileset: 4,
    width: 128,
    height: 128,
    terrainType: 0,
    players: defaultPlayers(),
    forces: defaultForces(),
    autoStart: true,
  };
}

export function validateProjectName(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "프로젝트 이름을 입력해 주세요.";
  if ([...trimmed].length > 64) return "프로젝트 이름은 64자 이하여야 합니다.";
  if (/[<>:"/\\|?*[\]\u0000-\u001f]/u.test(trimmed)) {
    return '프로젝트 이름에는 < > : " / \\ | ? * [ ] 문자를 쓸 수 없습니다.';
  }
  if (trimmed.endsWith(".")) return "프로젝트 이름은 마침표로 끝날 수 없습니다.";
  const stem = trimmed.split(".")[0]?.toUpperCase() ?? "";
  if (/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$/u.test(stem)) {
    return "Windows 예약 이름은 프로젝트 이름으로 쓸 수 없습니다.";
  }
  return null;
}

export function validateSize(value: number, min = MAP_SIZE_MIN, max = MAP_SIZE_MAX): string | null {
  if (!Number.isInteger(value)) return "맵 크기는 정수여야 합니다.";
  if (value < min || value > max) return `맵 크기는 ${min}~${max} 사이여야 합니다.`;
  return null;
}

/** Build the strict engine spec from the wizard form. */
export function buildMapNewSpec(form: WizardForm): MapNewSpec {
  const players12 = form.players.slice(0, TOTAL_SLOTS);
  const startsAt = (index: number, player: PlayerForm) =>
    form.autoStart && index < MAX_PLAYERS && slotNeedsStart(player.type);
  const startCount = players12.filter((player, index) => startsAt(index, player)).length;
  const preset = startLocationPreset(form.width, form.height, startCount);
  let nextStart = 0;
  const forces = form.forces.slice(0, MAX_FORCES).map((force, index) => ({
    name: forceLabel(force, index),
    allied: force.allied,
    alliedVictory: force.alliedVictory,
    sharedVision: force.sharedVision,
    randomStart: force.randomStart,
  }));
  const players = players12.map((player, index): MapNewPlayer => {
    const base: MapNewPlayer = { slot: index, type: player.type, race: player.race };
    if (index >= MAX_PLAYERS) return base;
    const withForce = { ...base, force: Math.max(0, Math.min(player.force, forces.length - 1)) };
    return startsAt(index, player) ? { ...withForce, start: preset[nextStart++] } : withForce;
  });
  const name = form.name.trim();
  const title = form.title.trim() === "" ? name : form.title.trim();
  return {
    version: form.version,
    tileset: form.tileset,
    width: form.width,
    height: form.height,
    terrainType: form.terrainType,
    title,
    description: form.description.trim(),
    players,
    forces,
  };
}

// ---- IPC ----

export class MapNewProtocolError extends Error {
  readonly name = "MapNewProtocolError";
  readonly command: string;

  constructor(command: string) {
    super(`invalid response from ${command}`);
    this.command = command;
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseOptions(value: unknown): MapNewOptions {
  if (
    !isObject(value) ||
    !isObject(value.starcraft) ||
    typeof value.starcraft.available !== "boolean" ||
    typeof value.starcraft.path !== "string" ||
    !Array.isArray(value.tilesets) ||
    !Array.isArray(value.sizePresets) ||
    typeof value.sizeMin !== "number" ||
    typeof value.sizeMax !== "number"
  ) {
    throw new MapNewProtocolError("setup_map_new_options");
  }
  const tilesets = value.tilesets.map((entry): TilesetOption => {
    if (
      !isObject(entry) ||
      typeof entry.id !== "number" ||
      typeof entry.key !== "string" ||
      typeof entry.label !== "string"
    ) {
      throw new MapNewProtocolError("setup_map_new_options");
    }
    return { id: entry.id, key: entry.key, label: entry.label };
  });
  const sizePresets = value.sizePresets.map((entry) => {
    if (typeof entry !== "number") throw new MapNewProtocolError("setup_map_new_options");
    return entry;
  });
  const starcraft: StarcraftAvailability = {
    available: value.starcraft.available,
    path: value.starcraft.path,
    ...(typeof value.starcraft.reason === "string" ? { reason: value.starcraft.reason } : {}),
  };
  return { starcraft, tilesets, sizePresets, sizeMin: value.sizeMin, sizeMax: value.sizeMax };
}

function parseBrushes(value: unknown): BrushOption[] {
  if (!Array.isArray(value)) throw new MapNewProtocolError("setup_map_new_brushes");
  return value.map((entry): BrushOption => {
    if (
      !isObject(entry) ||
      typeof entry.id !== "number" ||
      typeof entry.name !== "string" ||
      typeof entry.graphicsValid !== "boolean"
    ) {
      throw new MapNewProtocolError("setup_map_new_brushes");
    }
    return { id: entry.id, name: entry.name, graphicsValid: entry.graphicsValid };
  });
}

function parsePreview(value: unknown): BlankProjectPreview {
  if (
    !isObject(value) ||
    typeof value.mapPath !== "string" ||
    typeof value.outputMap !== "string" ||
    typeof value.width !== "number" ||
    typeof value.height !== "number" ||
    typeof value.tileset !== "string" ||
    typeof value.players !== "number" ||
    typeof value.startLocations !== "number" ||
    typeof value.previewPng !== "string"
  ) {
    throw new MapNewProtocolError("setup_create_blank_project");
  }
  return {
    mapPath: value.mapPath,
    outputMap: value.outputMap,
    width: value.width,
    height: value.height,
    tileset: value.tileset,
    players: value.players,
    startLocations: value.startLocations,
    previewPng: value.previewPng,
  };
}

function parseBlankProjectResult(value: unknown): BlankProjectResult {
  if (!isObject(value)) throw new MapNewProtocolError("setup_create_blank_project");
  const { preview, ...rest } = value;
  const candidate = { ...rest, type: "setup" };
  if (!isServerMessage(candidate) || candidate.type !== "setup") {
    throw new MapNewProtocolError("setup_create_blank_project");
  }
  return {
    setup: candidate,
    preview: preview === undefined || preview === null ? null : parsePreview(preview),
  };
}

export async function mapNewOptions(invoke: InvokeFn = tauriInvoke): Promise<MapNewOptions> {
  return parseOptions(await invoke("setup_map_new_options"));
}

export async function mapNewBrushes(
  tileset: number,
  invoke: InvokeFn = tauriInvoke,
): Promise<BrushOption[]> {
  return parseBrushes(await invoke("setup_map_new_brushes", { tileset }));
}

export async function pickStarcraftPath(invoke: InvokeFn = tauriInvoke): Promise<MapNewOptions> {
  return parseOptions(await invoke("setup_pick_starcraft_path"));
}

export async function createBlankProject(
  request: BlankProjectRequest,
  invoke: InvokeFn = tauriInvoke,
): Promise<BlankProjectResult> {
  return parseBlankProjectResult(await invoke("setup_create_blank_project", { request }));
}
