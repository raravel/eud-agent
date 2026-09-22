import { describe, expect, it } from "vitest";

import {
  applyForceLayout,
  defaultForces,
  defaultPlayers,
  forceAssignment,
  forceLabel,
  forceLayoutAvailable,
  forceMembers,
  layoutSlots,
  MAX_PLAYERS,
  TOTAL_SLOTS,
  withForcePatch,
  withPlayerForce,
  withPlayerPatch,
  type SlotsForm,
} from "@/lib/mapSlots";

function form(): SlotsForm {
  return { players: defaultPlayers(), forces: defaultForces() };
}

describe("slot defaults", () => {
  it("always holds 12 slots and 4 forces", () => {
    const base = form();
    expect(base.players).toHaveLength(TOTAL_SLOTS);
    expect(base.forces).toHaveLength(4);
    expect(base.players.every((player) => player.force === 0)).toBe(true);
  });
});

describe("force presets", () => {
  it("assigns forces per layout", () => {
    expect(forceAssignment("single", 5)).toEqual([0, 0, 0, 0, 0]);
    expect(forceAssignment("individual", 3)).toEqual([0, 1, 2]);
    expect(forceAssignment("teams", 5)).toEqual([0, 0, 0, 1, 1]);
    expect(forceAssignment("teams", 2)).toEqual([0, 1]);
  });

  it("counts only P1..P8 that are neither unused nor closed as layout slots", () => {
    let base = form();
    base = withPlayerPatch(base, 1, { type: "closed" });
    base = withPlayerPatch(base, 5, { type: "inactive" });
    base = withPlayerPatch(base, 8, { type: "human" });
    expect(layoutSlots(base.players)).toEqual([0, 2, 3, 4, 6, 7]);
    expect(forceLayoutAvailable("individual", layoutSlots(base.players).length)).toBe(false);
    expect(forceLayoutAvailable("teams", layoutSlots(base.players).length)).toBe(true);
  });

  it("splits the layout slots into teams and resets the other P1..P8 slots to force 1", () => {
    let base = form();
    base = withPlayerPatch(base, 1, { type: "closed", force: 3 });
    base = withPlayerPatch(base, 6, { type: "inactive", force: 2 });
    const teams = applyForceLayout(base, "teams");
    // Layout slots: P1, P3, P4, P5, P6, P8 → first three to force 1, last three to force 2.
    expect(teams.players.map((player) => player.force)).toEqual([0, 0, 0, 0, 1, 1, 0, 1, 0, 0, 0, 0]);
    expect(teams.forces).toBe(base.forces);
  });

  it("gives each of at most four layout slots its own force and leaves unavailable layouts untouched", () => {
    let base = form();
    for (const slot of [4, 5, 6, 7]) base = withPlayerPatch(base, slot, { type: "closed" });
    const individual = applyForceLayout(base, "individual");
    expect(individual.players.slice(0, 8).map((player) => player.force)).toEqual([0, 1, 2, 3, 0, 0, 0, 0]);

    const eight = form();
    expect(applyForceLayout(eight, "individual")).toBe(eight);
    const single = applyForceLayout(individual, "single");
    expect(single.players.every((player) => player.force === 0)).toBe(true);
  });
});

describe("free slot editing", () => {
  it("assigns P1..P8 to declared forces and ignores everything out of range", () => {
    let base = form();
    base = withPlayerForce(base, 0, 3);
    base = withPlayerForce(base, 7, 1);
    expect(base.players[0]?.force).toBe(3);
    expect(base.players[7]?.force).toBe(1);
    expect(withPlayerForce(base, MAX_PLAYERS, 1)).toBe(base);
    expect(withPlayerForce(base, 11, 1)).toBe(base);
    expect(withPlayerForce(base, 0, 4)).toBe(base);
    expect(withPlayerForce(base, 0, -1)).toBe(base);
    expect(withPlayerForce(base, 0, 1.5)).toBe(base);
  });

  it("patches type and race per slot while routing force through the same bounds", () => {
    let base = form();
    base = withPlayerPatch(base, 9, { type: "computer", race: "zerg", force: 2 });
    expect(base.players[9]).toEqual({ type: "computer", race: "zerg", force: 0 });
    base = withPlayerPatch(base, 2, { race: "protoss", force: 1 });
    expect(base.players[2]).toEqual({ type: "human", race: "protoss", force: 1 });
    expect(withPlayerPatch(base, 2, {})).toBe(base);
  });

  it("patches forces in place and lists P1..P8 members only", () => {
    let base = form();
    base = withForcePatch(base, 1, { name: "방어", allied: false });
    expect(base.forces[1]).toEqual({ name: "방어", allied: false, alliedVictory: true, sharedVision: false, randomStart: false });
    expect(withForcePatch(base, 4, { name: "x" })).toBe(base);
    base = withPlayerPatch(base, 1, { force: 1 });
    base = withPlayerPatch(base, 4, { force: 1 });
    expect(forceMembers(base, 1)).toEqual([1, 4]);
    expect(forceMembers(base, 0)).toEqual([0, 2, 3, 5, 6, 7]);
    expect(forceMembers(base, 2)).toEqual([]);
  });

  it("labels forces by name with a positional fallback", () => {
    expect(forceLabel({ ...defaultForces()[0]!, name: " 방어 " }, 2)).toBe("방어");
    expect(forceLabel({ ...defaultForces()[0]!, name: "   " }, 2)).toBe("포스 3");
  });
});
