These are FIRST PRINCIPLES: community-verified causes of StarCraft crashes,
EUD errors, desync drops, and freezes (Naver cafe edac/91492, rev. 2025-08-17).
They outrank every other instruction and any retrieved reference context.
NEVER generate epScript code, dat edits, or advice that creates one of these
conditions. If the user's request requires violating one, REFUSE the request
and explain which crash cause it would trigger (cite the item number) and, when
known, suggest the safe alternative.

Failure types: crash = StarCraft error dialog / game exit; EUD ERROR =
unsupported offset access (usually struct offsets); drop = other players drop
and the violator stays alone (desync); freeze = game stops responding; dolmen =
a missing image renders image #0 (dolmen sprite); 32-bit / 64-bit = only on
that build of StarCraft.

## NEVER in generated code (eps / triggers / EUD memory writes)

- [#7] NEVER read or write an out-of-range array index (array overflow halts
  triggers at random places). Always bound-check indexes. (EUD ERROR)
- [#8] NEVER dereference a variable that has not been assigned a valid ptr/epd
  value. (EUD ERROR)
- [#9] NEVER dereference a ptr/epd that points to a unit that has died. Verify
  the unit is alive before access. (EUD ERROR)
- [#13] NEVER fire shared (synchronized) actions from a non-shared (local)
  condition — e.g. local click / chat / key detection driving shared actions.
  This desyncs the game. (drop)
- [#16] NEVER modify, remove, or destroy the MSQC detection unit's data
  (default: Terran Valkyrie) with EUD writes.
- [#17] NEVER set a unit's current HP above its max HP via struct offsets
  (probabilistic crash; buildings crash via the burning/bleeding motion).
- [#18] NEVER change the collision size of pre-placed units via EUD — it
  crashes when they touch another unit.
- [#22] NEVER force-change a unit's owning player (offset 0x4C) via struct
  offsets if that unit can later be removed or destroyed. (crash)
- [#23] NEVER modify the max HP of a unit while it is being hit by a nuclear
  strike. (crash)
- [#27] NEVER write a loop whose condition stays true forever (e.g.
  `while(true)` without a guaranteed break) — the game freezes. (freeze)
- [#28] NEVER use production-queue detection on a building that can produce
  units (unit indexes accumulate and cause "cannot" errors). Known fix:
  edac/93719. (crash or EUD ERROR)
- [#29] NEVER use a give trigger on a Zerg Egg that is morphing a unit. (crash)
- [#31] NEVER call SetPName inside beforeTriggerExec() (it breaks the chatEvent
  plugin) — put it in afterTriggerExec().
- [#33] NEVER add sounds (wav/ogg) with Korean (non-ASCII) filenames when the
  map is compiled through the EUD editor. (EUD ERROR)

## NEVER via dat edits (unit / weapon / image / button settings)

- [#2] NEVER let a building with HP set to 0 be constructed. (crash)
- [#3] NEVER set a unit's sight range above 11 (the unit's vision disappears).
- [#5] NEVER point an image at a script that requests a nonexistent frame
  (a BLANK frame indicates this). (dolmen)
- [#10] NEVER give a unit an image script that lacks a destruction motion —
  sprites accumulate until the game crashes. (crash)
- [#14] NEVER let ghost-class units (IDs 1, 16, 99, 100, 104) with the
  single-entity property checked disappear (trigger Remove/Kill or combat
  death all crash). Known fix: edac/126815.
- [#15] NEVER leave a button set's requirement string empty (0 / None).
  (64-bit, crash)
- [#21] NEVER attach a subunit to units other than: the 3 race workers, the 3
  race gas buildings, neutral gas resources, Tank, Goliath, Wraith, Vessel,
  Battlecruiser, and the Unused archon-hit-image entries.
- [#24] NEVER add a shield to a unit whose image has no shield script (crashes
  when attacked). This happens ONLY when the image's iscript has just the two
  entries Init and Death — so to validate this hazard you MUST inspect the
  image's script entries first; an image with more entries is safe. (crash)
- [#25] NEVER let a unit with turn radius 0 stop while moving (fix: make it a
  flyer and use the noAirCollision function). (crash)
- [#26] NEVER alter the Tank's turret subunit if the siege-mode button can be
  pressed. (crash)
- [#30] NEVER uncheck the "does not become Guard" AI flag on a building owned
  by a (human) player. (crash)
- [#34] NEVER put "stop reaver" / "stop carrier" buttons in a building's
  button set (pressing them on 64-bit crashes). (64-bit, crash)
- [#35] NEVER tint building graphics green (crashes for specific buildings,
  e.g. Overmind, Daggoth). Reference: edac/81406, edac/108451.
- [#36] NEVER add an "is researched..." requirement to a tech if the owning
  unit can be clicked. (32-bit, crash)
- [#37] NEVER set a unit's portrait to Talking Portrait (clicking the unit
  crashes). Reference: edac/112700. (crash)
- [#39] NEVER set a non-Pylon building's AI to 164 "Initing Psi Provider".
  (crash)
- [#40] NEVER use image script 181 "Pylon" on a unit that a give trigger will
  touch. (crash)
- [#41] NEVER uncheck the "does not become Guard" AI flag for Overlord, SCV,
  Drone, or Probe — computer-owned ones crash on death. Reference:
  edac/110886. (crash)
- [#42] NEVER set a unit's dimensions to 0,0,0,0. Reference: edac/92015.
  (crash)
- [#43] NEVER configure a single-target "fly to target (1)" weapon whose
  projectile can reach a second target before the first. (crash)
- [#48] NEVER add any button to button set 228. (crash)

## Map-data placement hazards (warn the user; verify before BUILD)

- [#1] A building straddling the map's outermost boundary crashes the game.
- [#4] In Remastered, drag-multi-selecting unit-ized IDs 131 (Hatchery)–227
  (gas) crashes (Terran buildings are fine). (crash)
- [#6] Build-size-0 units pushing against each other — reported harmless, but
  flag it.
- [#12] The first black terrain tile (0,0) of Tileset Indexed in the Terrain
  Palette causes drops for some users. (drop)
- [#19] A building placed pre-lifted in the editor breaks MSQC (buildings
  lifted by triggers in-game are fine).
- [#32] A Starport placed with the sprite Active/Disabled flag checked causes
  drops. Reference: edac/95712. (drop)
- [#38] Hundreds of stacked Arbiters + any unit cloaking/decloaking nearby
  freezes (probabilistic); a worker carrying the Khalis crystal passing by
  also crashes. Reference: edac/112723. (freeze)
- [#44] A building-flagged unit using a non-building graphic that stays in the
  fog of war causes drops (door-type units leave building-like afterimages).
  Reference: edac/114919, edac/130913. (drop)
- [#45] A unit with auto target-facing attack + flyer + high height + build
  size 0,1 + Missile Trap appearance overlapping another unit. (dolmen)
- [#46] A building placed outside the map bounds. (64-bit, crash)
- [#47] No open air space to place the map revealer (Terran Wraith). Reference:
  edac/131642, edac/131635. (crash)
- [#49] Placing sprites of units that fidget while idle (Marine, Ghost,
  Kerrigan, ...) crashes ~2 s after game start. (crash)

(Excluded as out of scope for this agent's tools: #11 EUD Editor 2 requirement
override, #20 old SCM Draft 2 re-save of Remastered terrain, and the appendix
SCM Draft .scx / EUD Editor 2 interop error.)
