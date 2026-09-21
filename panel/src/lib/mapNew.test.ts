import { describe, expect, it, vi } from "vitest";

import {
  applyForceLayout,
  buildMapNewSpec,
  createBlankProject,
  defaultPlayer,
  defaultWizardForm,
  forceAssignment,
  forceLabel,
  forceLayoutAvailable,
  forceMembers,
  mapNewBrushes,
  mapNewOptions,
  MapNewProtocolError,
  startLocationPreset,
  validateProjectName,
  validateSize,
  withForceCount,
  withPlayerCount,
  withPlayerForce,
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

describe("force presets", () => {
  it("assigns forces per layout", () => {
    expect(forceAssignment("single", 5)).toEqual([0, 0, 0, 0, 0]);
    expect(forceAssignment("individual", 3)).toEqual([0, 1, 2]);
    expect(forceAssignment("teams", 5)).toEqual([0, 0, 0, 1, 1]);
    expect(forceAssignment("teams", 2)).toEqual([0, 1]);
  });

  it("reports unavailable layouts and leaves the form untouched for them", () => {
    expect(forceLayoutAvailable("individual", 5)).toBe(false);
    expect(forceAssignment("individual", 5)).toEqual([0, 0, 0, 0, 0]);
    expect(forceLayoutAvailable("teams", 1)).toBe(false);
    expect(forceAssignment("teams", 1)).toEqual([0]);
    const five = withPlayerCount(defaultWizardForm(), 5);
    expect(applyForceLayout(five, "individual")).toBe(five);
  });

  it("grows the force list for a preset while keeping edited names and extra forces", () => {
    const form = {
      ...withPlayerCount(defaultWizardForm(), 3),
      forces: [{ ...defaultWizardForm().forces[0]!, name: "공격" }],
    };
    const teams = applyForceLayout(form, "teams");
    expect(teams.forces.map((force) => force.name)).toEqual(["공격", "포스 2"]);
    expect(teams.players.map((player) => player.force)).toEqual([0, 0, 1]);

    const single = applyForceLayout(withForceCount(teams, 4), "single");
    expect(single.forces).toHaveLength(4);
    expect(single.players.map((player) => player.force)).toEqual([0, 0, 0]);
  });
});

describe("free force editing", () => {
  it("bounds the force count to 1..4 and keeps players on declared forces", () => {
    let form = withPlayerCount(defaultWizardForm(), 4);
    form = withForceCount(form, 9);
    expect(form.forces).toHaveLength(4);
    form = withPlayerForce(form, 0, 3);
    form = withPlayerForce(form, 1, 2);
    form = withPlayerForce(form, 2, 1);
    expect(form.players.map((player) => player.force)).toEqual([3, 2, 1, 0]);

    const shrunk = withForceCount(form, 2);
    expect(shrunk.forces).toHaveLength(2);
    expect(shrunk.players.map((player) => player.force)).toEqual([1, 1, 1, 0]);
    expect(withForceCount(shrunk, 0).forces).toHaveLength(1);
  });

  it("ignores out-of-range player force targets", () => {
    const form = withForceCount(defaultWizardForm(), 2);
    expect(withPlayerForce(form, 0, 2)).toBe(form);
    expect(withPlayerForce(form, 0, -1)).toBe(form);
    expect(withPlayerForce(form, 0, 1.5)).toBe(form);
  });

  it("adds new players to force 1 and lists members per force", () => {
    let form = withForceCount(defaultWizardForm(), 3);
    form = withPlayerForce(form, 1, 2);
    form = withPlayerCount(form, 4);
    expect(form.players.map((player) => player.force)).toEqual([0, 2, 0, 0]);
    expect(forceMembers(form, 0)).toEqual([0, 2, 3]);
    expect(forceMembers(form, 1)).toEqual([]);
    expect(forceMembers(form, 2)).toEqual([1]);
    expect(withPlayerCount(form, 2).players).toEqual([defaultPlayer(), defaultPlayer(2)]);
  });

  it("labels forces by name with a positional fallback", () => {
    expect(forceLabel({ ...defaultWizardForm().forces[0]!, name: " 방어 " }, 2)).toBe("방어");
    expect(forceLabel({ ...defaultWizardForm().forces[0]!, name: "   " }, 2)).toBe("포스 3");
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
  it("derives slots, forces, title fallback and start locations", () => {
    const form = {
      ...defaultWizardForm(),
      name: " 새 맵 ",
      width: 96,
      height: 64,
      terrainType: 3,
      players: [
        { type: "human" as const, race: "terran" as const, force: 0 },
        { type: "computer" as const, race: "zerg" as const, force: 2 },
        { type: "human" as const, race: "userSelectable" as const, force: 1 },
      ],
      forces: [
        { ...defaultWizardForm().forces[0]!, name: "공격" },
        { ...defaultWizardForm().forces[0]!, name: "  " },
        { ...defaultWizardForm().forces[0]!, name: "관전", allied: false },
      ],
    };
    const spec = buildMapNewSpec(form);
    expect(spec.title).toBe("새 맵");
    expect(spec.description).toBe("");
    expect(spec.forces.map((force) => force.name)).toEqual(["공격", "포스 2", "관전"]);
    expect(spec.forces[2]?.allied).toBe(false);
    expect(spec.players.map((player) => player.slot)).toEqual([0, 1, 2]);
    expect(spec.players.map((player) => player.force)).toEqual([0, 2, 1]);
    expect(spec.players.every((player) => player.start !== undefined)).toBe(true);
    expect(spec.players[1]?.start).toEqual({ x: 320, y: 176 });
  });

  it("skips closed and neutral slots when placing start locations", () => {
    const spec = buildMapNewSpec({
      ...defaultWizardForm(),
      name: "x",
      width: 64,
      height: 64,
      players: [
        { type: "human" as const, race: "terran" as const, force: 0 },
        { type: "closed" as const, race: "userSelectable" as const, force: 0 },
        { type: "computer" as const, race: "zerg" as const, force: 0 },
        { type: "neutral" as const, race: "userSelectable" as const, force: 0 },
      ],
    });
    expect(spec.players.map((player) => player.start)).toEqual([
      { x: 192, y: 176 },
      undefined,
      { x: 320, y: 176 },
      undefined,
    ]);
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
