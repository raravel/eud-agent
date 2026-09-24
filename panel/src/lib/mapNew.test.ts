import { describe, expect, it, vi } from "vitest";

import {
  buildMapNewSpec,
  createBlankProject,
  defaultWizardForm,
  mapNewBrushes,
  mapNewOptions,
  MapNewProtocolError,
  MAX_FORCES,
  startLocationPreset,
  TOTAL_SLOTS,
  validateProjectName,
  validateSize,
  withForcePatch,
  withPlayerPatch,
} from "@/lib/mapNew";
import { PROVIDER_IDS, type ProviderStatus } from "@/providers/types";

const providers: ProviderStatus[] = PROVIDER_IDS.map((provider) => ({
  provider,
  availability: "unavailable",
  selectedAsDefault: false,
  canInstall: false,
  canImport: false,
  experimental: false,
}));

describe("startLocationPreset", () => {
  it("packs every start location into the top-left corner, four per row, without overlap", () => {
    expect(startLocationPreset(128, 96, 8)).toEqual([
      { x: 192, y: 176 },
      { x: 320, y: 176 },
      { x: 448, y: 176 },
      { x: 576, y: 176 },
      { x: 192, y: 272 },
      { x: 320, y: 272 },
      { x: 448, y: 272 },
      { x: 576, y: 272 },
    ]);
  });

  it("returns exactly the requested count and caps at eight", () => {
    expect(startLocationPreset(64, 64, 0)).toEqual([]);
    expect(startLocationPreset(64, 64, 2)).toHaveLength(2);
    expect(startLocationPreset(256, 256, 12)).toHaveLength(8);
  });

  it("stays inside the margin on the smallest map", () => {
    for (const start of startLocationPreset(64, 64, 8)) {
      expect(start.x).toBeGreaterThanOrEqual(192);
      expect(start.x).toBeLessThanOrEqual(64 * 32 - 192);
      expect(start.y).toBeGreaterThanOrEqual(176);
      expect(start.y).toBeLessThanOrEqual(64 * 32 - 176);
    }
  });
});

describe("wizard slot defaults", () => {
  it("opens P1..P8 to humans, leaves P9..P11 unused and keeps P12 neutral", () => {
    const form = defaultWizardForm();
    expect(form.players).toHaveLength(TOTAL_SLOTS);
    expect(form.forces).toHaveLength(MAX_FORCES);
    expect(form.players.slice(0, 8).every((player) => player.type === "human" && player.race === "userSelectable")).toBe(true);
    expect(form.players.slice(8, 11).every((player) => player.type === "inactive" && player.race === "inactive")).toBe(true);
    expect(form.players[11]).toEqual({ type: "neutral", race: "neutral", force: 0 });
    expect(form.forces.map((force) => force.name)).toEqual(["포스 1", "포스 2", "포스 3", "포스 4"]);
  });
});

describe("validation", () => {
  it("mirrors the backend project-name rules", () => {
    expect(validateProjectName("Arena")).toBeNull();
    expect(validateProjectName("한글 맵")).toBeNull();
    expect(validateProjectName("")).not.toBeNull();
    expect(validateProjectName("a/b")).not.toBeNull();
    expect(validateProjectName("a[b]")).not.toBeNull();
    expect(validateProjectName("dot.")).not.toBeNull();
    expect(validateProjectName("CON")).not.toBeNull();
    expect(validateProjectName("x".repeat(65))).not.toBeNull();
  });

  it("bounds map sizes", () => {
    expect(validateSize(64)).toBeNull();
    expect(validateSize(256)).toBeNull();
    expect(validateSize(63)).not.toBeNull();
    expect(validateSize(257)).not.toBeNull();
    expect(validateSize(100.5)).not.toBeNull();
  });
});

describe("buildMapNewSpec", () => {
  it("emits all 12 slots, forces only for P1..P8, title fallback and start locations", () => {
    let form = { ...defaultWizardForm(), name: " 새 맵 ", width: 96, height: 64, terrainType: 3 };
    form = withPlayerPatch(form, 0, { race: "terran" });
    form = withPlayerPatch(form, 1, { type: "computer", race: "zerg", force: 2 });
    form = withPlayerPatch(form, 2, { force: 1 });
    form = withForcePatch(form, 0, { name: "공격" });
    form = withForcePatch(form, 1, { name: "  " });
    form = withForcePatch(form, 2, { name: "관전", allied: false });
    const spec = buildMapNewSpec(form);
    expect(spec.title).toBe("새 맵");
    expect(spec.description).toBe("");
    expect(spec.forces.map((force) => force.name)).toEqual(["공격", "포스 2", "관전", "포스 4"]);
    expect(spec.forces[2]?.allied).toBe(false);
    expect(spec.players.map((player) => player.slot)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    expect(spec.players.slice(0, 8).map((player) => player.force)).toEqual([0, 2, 1, 0, 0, 0, 0, 0]);
    expect(spec.players.slice(8).every((player) => !("force" in player) && !("start" in player))).toBe(true);
    expect(spec.players.slice(8).map((player) => [player.type, player.race])).toEqual([
      ["inactive", "inactive"],
      ["inactive", "inactive"],
      ["inactive", "inactive"],
      ["neutral", "neutral"],
    ]);
    expect(spec.players.slice(0, 8).every((player) => player.start !== undefined)).toBe(true);
    expect(spec.players[1]?.start).toEqual({ x: 320, y: 176 });
  });

  it("skips closed, unused and neutral slots when placing start locations", () => {
    let form = { ...defaultWizardForm(), name: "x", width: 64, height: 64 };
    form = withPlayerPatch(form, 0, { race: "terran" });
    form = withPlayerPatch(form, 1, { type: "closed" });
    form = withPlayerPatch(form, 2, { type: "computer", race: "zerg" });
    form = withPlayerPatch(form, 3, { type: "neutral" });
    form = withPlayerPatch(form, 4, { type: "inactive" });
    const spec = buildMapNewSpec(form);
    expect(spec.players.slice(0, 5).map((player) => player.start)).toEqual([
      { x: 192, y: 176 },
      undefined,
      { x: 320, y: 176 },
      undefined,
      undefined,
    ]);
    expect(spec.players[5]?.start).toEqual({ x: 448, y: 176 });
  });

  it("never gives P9..P12 a start location even when they are playable", () => {
    const form = withPlayerPatch({ ...defaultWizardForm(), name: "x" }, 8, { type: "human", race: "terran" });
    const spec = buildMapNewSpec(form);
    expect(spec.players[8]).toEqual({ slot: 8, type: "human", race: "terran" });
    expect(spec.players.filter((player) => player.start !== undefined)).toHaveLength(8);
  });

  it("omits start locations when auto placement is off", () => {
    const spec = buildMapNewSpec({ ...defaultWizardForm(), name: "x", autoStart: false });
    expect(spec.players.every((player) => player.start === undefined)).toBe(true);
    expect("start" in (spec.players[0] ?? {})).toBe(false);
  });
});

describe("IPC parsing", () => {
  it("parses options and brushes strictly", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command === "setup_map_new_options") {
        return {
          starcraft: { available: false, path: "", reason: "missing" },
          tilesets: [{ id: 0, key: "badlands", label: "배드랜드" }],
          sizePresets: [64, 128],
          sizeMin: 64,
          sizeMax: 256,
        };
      }
      return [{ id: 2, name: "Dirt", graphicsValid: true }];
    });
    const options = await mapNewOptions(invoke);
    expect(options.starcraft.reason).toBe("missing");
    expect(options.tilesets[0]?.label).toBe("배드랜드");
    expect(await mapNewBrushes(0, invoke)).toEqual([{ id: 2, name: "Dirt", graphicsValid: true }]);
    expect(invoke).toHaveBeenLastCalledWith("setup_map_new_brushes", { tileset: 0 });

    await expect(mapNewOptions(async () => ({ tilesets: [] }))).rejects.toBeInstanceOf(
      MapNewProtocolError,
    );
    await expect(mapNewBrushes(0, async () => [{ id: "x" }])).rejects.toBeInstanceOf(
      MapNewProtocolError,
    );
  });

  it("splits the flattened setup status from the preview", async () => {
    const request = {
      destination: "C:\\Work\\Arena",
      name: "Arena",
      spec: buildMapNewSpec({ ...defaultWizardForm(), name: "Arena", terrainType: 3 }),
    };
    const invoke = vi.fn(async () => ({
      projectPath: "C:\\Work\\Arena",
      projectValid: true,
      projectOpened: true,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      providers,
      setupRequired: true,
      preview: {
        mapPath: "C:\\Work\\Arena\\maps\\Arena.scx",
        outputMap: "build/[EUD]Arena.scx",
        width: 128,
        height: 128,
        tileset: "jungle",
        players: 2,
        startLocations: 2,
        previewPng: "iVBORw0KGgo=",
      },
    }));
    const result = await createBlankProject(request, invoke);
    expect(invoke).toHaveBeenCalledWith("setup_create_blank_project", { request });
    expect(result.setup.type).toBe("setup");
    expect(result.setup.projectOpened).toBe(true);
    expect(result.preview?.outputMap).toBe("build/[EUD]Arena.scx");

    const cancelled = await createBlankProject(request, async () => ({
      projectPath: "",
      projectValid: false,
      projectOpened: false,
      euddraftPath: "",
      euddraftValid: false,
      assetsReady: false,
      providers,
      setupRequired: true,
      error: "project_create_failed: x",
    }));
    expect(cancelled.preview).toBeNull();
    expect(cancelled.setup.error).toBe("project_create_failed: x");
  });
});
