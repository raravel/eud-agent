import { describe, expect, it } from "vitest";

import {
  buildMapPropertiesRequest,
  propertiesFormEqual,
  propertiesFormFromDigest,
  raceFromRaceId,
  slotTypeFromControllerId,
  validateMapProperties,
} from "./mapProperties";
import type { MapDigest, MapDigestForce, MapDigestPlayer } from "./mapProtocol";

const CONTROLLERS = [6, 5, 3, 7, 0, 8, 1, 2, 0, 0, 4, 7];
const RACES = [1, 0, 2, 4, 7, 5, 6, 3, 7, 7, 7, 4];
const FORCES = [1, 1, 2, 2, 3, 4, 4, 1];

function digestPlayers(): MapDigestPlayer[] {
  return CONTROLLERS.map((controllerId, slot) => ({
    player: `P${slot + 1}`,
    controller: String(controllerId),
    controllerId,
    race: String(RACES[slot]),
    raceId: RACES[slot]!,
    ...(slot < 8 ? { force: FORCES[slot] } : {}),
  }));
}

function digestForces(): MapDigestForce[] {
  return [1, 2, 3, 4].map((force) => ({
    force,
    name: force === 2 ? "방어" : `Force ${force}`,
    players: [],
    flags: {
      randomStartLocation: force === 3,
      allies: force !== 4,
      alliedVictory: force === 1,
      sharedVision: force === 2,
    },
  }));
}

function digest(overrides: Partial<MapDigest> = {}): MapDigest {
  return {
    map: { width: 128, height: 128, tileset: "jungle", title: "협동 방어전", description: "설명" },
    units: [],
    doodads: [],
    sprites: [],
    locations: [],
    startLocations: [],
    players: digestPlayers(),
    forces: digestForces(),
    ...overrides,
  };
}

describe("map id mapping", () => {
  it("maps every OWNR controller id to a slot type, folding legacy ids onto the nearest choice", () => {
    expect([0, 1, 2, 3, 4, 5, 6, 7, 8].map(slotTypeFromControllerId)).toEqual([
      "inactive",
      "computer",
      "human",
      "rescuable",
      "inactive",
      "computer",
      "human",
      "neutral",
      "closed",
    ]);
  });

  it("maps SIDE race ids and keeps independent as a display-only value", () => {
    expect([0, 1, 2, 3, 4, 5, 6, 7].map(raceFromRaceId)).toEqual([
      "zerg",
      "terran",
      "protoss",
      "independent",
      "neutral",
      "userSelectable",
      "random",
      "inactive",
    ]);
  });
});

describe("propertiesFormFromDigest", () => {
  it("builds title, 12 slots with 0-based forces and 4 forces from the digest", () => {
    const form = propertiesFormFromDigest(digest());
    expect(form).not.toBeNull();
    expect(form!.title).toBe("협동 방어전");
    expect(form!.description).toBe("설명");
    expect(form!.players).toHaveLength(12);
    expect(form!.players.map((player) => player.type)).toEqual([
      "human", "computer", "rescuable", "neutral", "inactive", "closed", "computer", "human",
      "inactive", "inactive", "inactive", "neutral",
    ]);
    expect(form!.players.map((player) => player.race)).toEqual([
      "terran", "zerg", "protoss", "neutral", "inactive", "userSelectable", "random", "independent",
      "inactive", "inactive", "inactive", "neutral",
    ]);
    expect(form!.players.map((player) => player.force)).toEqual([0, 0, 1, 1, 2, 3, 3, 0, 0, 0, 0, 0]);
    expect(form!.forces.map((force) => force.name)).toEqual(["Force 1", "방어", "Force 3", "Force 4"]);
    expect(form!.forces[1]).toEqual({
      name: "방어",
      allied: true,
      alliedVictory: false,
      sharedVision: true,
      randomStart: false,
    });
    expect(form!.forces[3]?.allied).toBe(false);
    expect(form!.forces[2]?.randomStart).toBe(true);
  });

  it("refuses digests that predate map properties instead of inventing defaults", () => {
    expect(propertiesFormFromDigest(digest({ players: undefined }))).toBeNull();
    expect(propertiesFormFromDigest(digest({ forces: undefined }))).toBeNull();
  });

  it("falls back to an unused slot and a default force for entries the digest leaves out", () => {
    const form = propertiesFormFromDigest(
      digest({ players: digestPlayers().slice(0, 2), forces: digestForces().slice(0, 1), map: { width: 1, height: 1, tileset: "jungle" } }),
    );
    expect(form!.title).toBe("");
    expect(form!.players[5]).toEqual({ type: "inactive", race: "inactive", force: 0 });
    expect(form!.forces[3]?.name).toBe("포스 4");
  });
});

describe("request encoding", () => {
  it("emits force only for P1..P8, trims text and echoes a read-only independent race", () => {
    const form = propertiesFormFromDigest(digest())!;
    const request = buildMapPropertiesRequest({ ...form, title: " 제목 ", description: " 설명 " });
    expect(request.title).toBe("제목");
    expect(request.description).toBe("설명");
    expect(request.players).toHaveLength(12);
    expect(request.players.slice(0, 8).map((player) => player.force)).toEqual([0, 0, 1, 1, 2, 3, 3, 0]);
    expect(request.players.slice(8).every((player) => !("force" in player))).toBe(true);
    expect(request.players[7]?.race).toBe("independent");
    expect(request.players[6]).toEqual({ type: "computer", race: "random", force: 3 });
    expect(request.forces).toHaveLength(4);
    expect(request.forces[1]).toEqual({ name: "방어", allied: true, alliedVictory: false, sharedVision: true, randomStart: false });
  });

  it("detects changes field by field and validates required names", () => {
    const form = propertiesFormFromDigest(digest())!;
    expect(propertiesFormEqual(form, propertiesFormFromDigest(digest())!)).toBe(true);
    expect(propertiesFormEqual(form, { ...form, description: "x" })).toBe(false);
    expect(
      propertiesFormEqual(form, {
        ...form,
        players: form.players.map((player, index) => (index === 3 ? { ...player, force: 2 } : player)),
      }),
    ).toBe(false);
    expect(
      propertiesFormEqual(form, {
        ...form,
        forces: form.forces.map((force, index) => (index === 0 ? { ...force, sharedVision: true } : force)),
      }),
    ).toBe(false);
    expect(validateMapProperties(form)).toBeNull();
    expect(validateMapProperties({ ...form, title: "  " })).toBe("맵 제목을 입력해 주세요.");
    expect(
      validateMapProperties({ ...form, forces: form.forces.map((force, index) => (index === 2 ? { ...force, name: "" } : force)) }),
    ).toBe("포스 이름을 모두 입력해 주세요.");
  });
});
