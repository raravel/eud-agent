---
completed_at: '2026-06-06T18:12:32.752699'
created: '2026-06-06'
depends_on: []
id: EUD-084-1bff
priority: medium
scope:
- server/eud_agent/chk_info.py
- server/eud_agent/tools.py
- server/eud_agent/journal.py
- server/eud_agent/app.py
- server/tests/test_chk_info.py
- hivemind/docs/
status: done
title: 'location_write tool: agent-driven MRGN CRUD on the source map via IsomTerrain
  locedit (backup + plan gate + lock check)'
type: task
updated: '2026-06-06'
---

User decision (clarify 2026-06-06): route 3 (write the source .scx), full CRUD,
max safety (backup + approval + lock check).

isom-poc (NOT a git repo; files edited in place): MapGenCli.cpp `locedit
<map> <ops>` subcommand — pipe-separated ops, px coords, raw name bytes,
all-or-nothing apply, save with autoDefragmentLocations=false (NEVER renumber
location ids), #64 Anywhere protected, del refused when map triggers reference
the slot. Rebuilt ReleaseUS x64; smoke + headless E2E on hill_demo copies.

eud-agent: MapInfoService.location_write (compiling guard -> CreateFileW
no-share lock probe -> full-file backup under data_dir/map_backups -> locedit
spawn -> post-edit digest verify; Korean names follow the map's STR encoding);
location_write WRITE ToolSpec (plan-gated, budgeted, journaled with
before={mapPath,backupPath}); journal._rollback_location restores the backup
bytes (changeset reject); docs features/09 + architecture + rules "Map file
writes" section.

Verified: 682 passed/4 skipped (15 new), ruff clean for changed files,
real-exe E2E: add 공격지점 -> set -> rename -> add -> delete -> restore, ids
stable throughout. SCMDraft visual confirmation remains user-assisted.

## Completion Criteria
- [x] locedit never renumbers ids; Anywhere protected; all-or-nothing
- [x] service rails: compiling guard, lock probe, backup, encoding-follow
- [x] tool plan-gated+journaled; changeset reject restores backup
- [x] full suite + real-exe headless E2E pass