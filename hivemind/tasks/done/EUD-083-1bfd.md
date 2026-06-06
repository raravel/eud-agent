---
completed_at: '2026-06-06T17:45:00.389775'
created: '2026-06-06'
depends_on: []
id: EUD-083-1bfd
priority: medium
scope:
- server/eud_agent/chk_info.py
- server/eud_agent/tools.py
- server/eud_agent/config.py
- server/eud_agent/app.py
- server/eud_agent/data/unit_names.json
- server/tests/test_chk_info.py
- hivemind/docs/
status: done
title: 'map_info tool: locations/units/forces digest of the connected map via IsomTerrain
  chk + Python CHK parser'
type: task
updated: '2026-06-06'
---

Feasibility review (2026-06-06) option B, executed directly in-session.

The editor exposes only the OpenMapName path string, so the connected map's
SCMD2-authored data is read from disk: IsomTerrain.exe chk (verified isom-poc
CLI, zero C++ changes) extracts the raw CHK; server/eud_agent/chk_info.py
parses MRGN/UNIT/FORC/OWNR/SIDE/DIM/ERA/STR(+STRx) in Python and the map_info
READ tool (tools.py routing, memory_write precedent) slices it into
summary|locations|units|players modes with owner/unitType filters and a
200-entry cap. Config: ISOMTERRAIN_CMD env > agent.cfg isomterrain_cmd >
built-in isom-poc build path; advisory shape (missing exe degrades only this
tool). Docs: features/08_map-info-tool.md + architecture.md.

Verified: 667 passed/4 skipped (26 new in test_chk_info.py), ruff clean for
changed files (3 pre-existing errors untouched), selfcheck OK, live digest of
isom-poc hill_demo.scx through the real exe correct.

## Completion Criteria
- [x] map_info registered as READ tool; service-absent/misconfig is a clear ToolError
- [x] CHK parser covers MRGN/UNIT/FORC/OWNR/SIDE/DIM/ERA/STR+STRx incl. cp949 names
- [x] verify stages lint(test scope)/test/smoke pass headless