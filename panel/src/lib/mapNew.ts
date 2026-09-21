import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type { InvokeFn } from "@/lib/ipc";
import { isServerMessage, type SetupMessage } from "@/lib/protocol";

// ---- wire types (mirror src-tauri/src/blank_project.rs + crates/isom MapNewSpec) ----

export type MapVersion = "remastered" | "broodWar";
export type SlotType = "human" | "computer" | "rescuable" | "neutral" | "inactive" | "closed";
export type Race = "zerg" | "terran" | "protoss" | "userSelectable" | "random";

export interface MapNewStart {
  readonly x: number;
  readonly y: number;
}

export interface MapNewPlayer {
  readonly slot: number;
  readonly type: SlotType;
  readonly race: Race;
  readonly force: number;
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
export const MAX_PLAYERS = 8;
export const MAX_FORCES = 4;
/** Tiles kept clear between the map edge and an auto-placed start location. */
export const START_MARGIN_TILES = 4;
/** Start location footprint is 4x3 tiles; the CHK stores its centre in pixels. */
const START_HALF_WIDTH_PX = 64;
const START_HALF_HEIGHT_PX = 48;
const TILE_PX = 32;
/** Auto-placed start locations are packed four per row in the top-left corner. */
const START_CLUSTER_COLUMNS = 4;

export const SLOT_TYPE_OPTIONS: readonly { value: SlotType; label: string }[] = [
  { value: "human", label: "사람" },
  { value: "computer", label: "컴퓨터" },
  { value: "rescuable", label: "구출 가능" },
  { value: "neutral", label: "중립" },
  { value: "closed", label: "닫힘" },
];

export const RACE_OPTIONS: readonly { value: Race; label: string }[] = [
  { value: "userSelectable", label: "선택 가능" },
  { value: "terran", label: "테란" },
  { value: "zerg", label: "저그" },
  { value: "protoss", label: "프로토스" },
  { value: "random", label: "랜덤" },
];

export const VERSION_OPTIONS: readonly { value: MapVersion; label: string; hint: string }[] = [
  { value: "remastered", label: "리마스터 (.scx)", hint: "StarCraft: Remastered 기본 형식" },
  { value: "broodWar", label: "브루드 워 (.scx)", hint: "VER 205 형식 · 문자열은 SC:R 기준 UTF-8" },
];

export type ForceLayout = "single" | "individual" | "teams";

/** Quick presets that rewrite every player's force; the result stays freely editable. */
export const FORCE_LAYOUT_OPTIONS: readonly { value: ForceLayout; label: string; hint: string }[] = [
  { value: "single", label: "모두 한 포스", hint: "전원이 포스 1에 속합니다" },
  { value: "individual", label: "각자 개별 포스", hint: "플레이어마다 포스 하나 (최대 4명)" },
  { value: "teams", label: "2팀으로 나누기", hint: "앞 절반은 포스 1, 뒤 절반은 포스 2" },
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

export function forceLayoutAvailable(layout: ForceLayout, playerCount: number): boolean {
  if (layout === "individual") return playerCount <= MAX_FORCES;
  if (layout === "teams") return playerCount >= 2;
  return true;
}

/** Force index per player position (0-based) for a layout. */
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

export interface PlayerForm {
  readonly type: SlotType;
  readonly race: Race;
  /** 0-based index into `WizardForm.forces`. */
  readonly force: number;
}

export interface ForceForm {
  readonly name: string;
  readonly allied: boolean;
  readonly alliedVictory: boolean;
  readonly sharedVision: boolean;
  readonly randomStart: boolean;
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
  readonly players: readonly PlayerForm[];
  /** 1..4 forces; every player's `force` indexes this list. */
  readonly forces: readonly ForceForm[];
  readonly autoStart: boolean;
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

export function defaultPlayer(force = 0): PlayerForm {
  return { type: "human", race: "userSelectable", force };
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
    players: [defaultPlayer(), defaultPlayer()],
    forces: [defaultForce(0)],
    autoStart: true,
  };
}

/**
 * Resize the force list to `count` (1..4), keeping edited forces. Players that pointed at a
 * removed force move to the last remaining one so every slot still names a declared force.
 */
export function withForceCount(form: WizardForm, count: number): WizardForm {
  const needed = Math.max(1, Math.min(MAX_FORCES, Math.trunc(count)));
  const forces = Array.from({ length: needed }, (_, index) => form.forces[index] ?? defaultForce(index));
  const players = form.players.map((player) => (player.force < needed ? player : { ...player, force: needed - 1 }));
  return { ...form, forces, players };
}

/** Resize the player list, adding new slots to force 1 and keeping existing assignments. */
export function withPlayerCount(form: WizardForm, count: number): WizardForm {
  const needed = Math.max(1, Math.min(MAX_PLAYERS, Math.trunc(count)));
  const players = Array.from({ length: needed }, (_, index) => form.players[index] ?? defaultPlayer());
  return { ...form, players };
}

/** Assign one player to a declared force. Out-of-range targets leave the form unchanged. */
export function withPlayerForce(form: WizardForm, playerIndex: number, force: number): WizardForm {
  if (!Number.isInteger(force) || force < 0 || force >= form.forces.length) return form;
  return {
    ...form,
    players: form.players.map((player, index) => (index === playerIndex ? { ...player, force } : player)),
  };
}

/** Apply a quick preset: grows the force list as needed and rewrites every player's force. */
export function applyForceLayout(form: WizardForm, layout: ForceLayout): WizardForm {
  if (!forceLayoutAvailable(layout, form.players.length)) return form;
  const assignment = forceAssignment(layout, form.players.length);
  const needed = assignment.length === 0 ? 1 : Math.max(...assignment) + 1;
  const grown = withForceCount(form, Math.max(needed, form.forces.length));
  return {
    ...grown,
    players: grown.players.map((player, index) => ({ ...player, force: assignment[index] ?? 0 })),
  };
}

/** 0-based player indexes currently assigned to `force`. */
export function forceMembers(form: WizardForm, force: number): number[] {
  return form.players.flatMap((player, index) => (player.force === force ? [index] : []));
}

/** Display label for a force: its trimmed name, or the positional fallback when blank. */
export function forceLabel(force: ForceForm, index: number): string {
  const name = force.name.trim();
  return name === "" ? `포스 ${index + 1}` : name;
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

/** Slot types that actually play and therefore receive a start location. */
export function slotNeedsStart(type: SlotType): boolean {
  return type === "human" || type === "computer" || type === "rescuable";
}

/** Build the strict engine spec from the wizard form. */
export function buildMapNewSpec(form: WizardForm): MapNewSpec {
  const startCount = form.autoStart ? form.players.filter((player) => slotNeedsStart(player.type)).length : 0;
  const preset = startLocationPreset(form.width, form.height, startCount);
  let nextStart = 0;
  const forces = form.forces.slice(0, MAX_FORCES).map((force, index) => ({
    name: forceLabel(force, index),
    allied: force.allied,
    alliedVictory: force.alliedVictory,
    sharedVision: force.sharedVision,
    randomStart: force.randomStart,
  }));
  const players = form.players.map((player, index) => {
    const start = form.autoStart && slotNeedsStart(player.type) ? preset[nextStart++] : undefined;
    const base: MapNewPlayer = {
      slot: index,
      type: player.type,
      race: player.race,
      force: Math.max(0, Math.min(player.force, forces.length - 1)),
    };
    return start === undefined ? base : { ...base, start };
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
