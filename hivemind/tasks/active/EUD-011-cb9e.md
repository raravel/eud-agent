---
created: '2026-06-04'
depends_on:
- EUD-008-cddc
id: EUD-011-cb9e
parent: EUD-003-6e4e
priority: high
scope:
- bridge/ZZZ_10_agent_bridge.lua
- server/tests/test_bridge_list_static.py
- server/tests/test_imported_artifacts.py
status: in_progress
title: 'Bridge: LIST command (paths + file types)'
type: task
updated: '2026-06-04'
---

## Description
Add the LIST command to the bridge: walk pj.TEData.PFIles with the existing walk() helper, return one line per file as path TAB EFileType-name. No contents, no disk writes. Before wiring, verify the exact file-type member on a TEFile object on Windows via the LUA debug command (expected enum property; read with safestr). Return ERROR: no project when pjData is nil.

## Spec References
- [[features/01_lua-bridge|01_lua-bridge]] `../docs/features/01_lua-bridge.md` - New command: LIST
- [[architecture]] `../docs/architecture.md` - File IPC protocol table
- [[rules]] `../docs/rules.md` - Lua bridge crash rules (get_ properties, arr indexer, pcall limits)

## Completion Criteria
- [ ] LIST inbox command returns all project files with correct paths and type names
- [ ] Verified on Windows editor via inbox/outbox round-trip (paste result in task notes)
- [ ] No .NET-exception-prone calls (follows rules.md luanet rules)
- [ ] v6 commands still answer (PING regression)