# SCMDraft 2 doodad save fixtures

Generated through the real ScmDraft 2.0 Beta 0.9.10 (2019-09-24) UI on
2026-10-03, using a new 64x64 Jungle World map with Dirt terrain.

- `scmd2_doodad_add.scx`: Dirt #1 (DD2 id 75), center (544, 320) pixels.
- `scmd2_doodad_move.scx`: the same doodad moved by Edit Cut/Paste to (736, 544).
- `scmd2_doodad_delete.scx`: moved doodad removed with Edit Delete.

Add and move each have four MTXM/TILE differences. TILE retains the original
ground; MTXM contains the CV5 doodad footprint. Delete restores MTXM to TILE.
Reopening and saving the added map preserves MTXM, TILE, DD2, THG2 and ISOM.
These are synthetic test maps, with no user map data.
