---
completed_at: '2026-06-09T15:49:57.546412'
created: '2026-06-06'
depends_on:
- EUD-076-d2c2
id: EUD-077-de1e
parent: EUD-075-43ae
priority: medium
status: done
title: Memory WS endpoints + panel view
type: story
updated: '2026-06-09'
---

## Description
Story: user-facing surface of project memory — `memory_get`/`memory_save` WS endpoints and
the panel memory view (file tabs, Monaco markdown editing, episodes list, changeset
rendering for `memory` items).

## Spec References
- [[features/07_project-memory|07_project-memory]] `../docs/features/07_project-memory.md` — WS protocol additions, panel memory view
- [[features/03_agent-panel|03_agent-panel]] `../docs/features/03_agent-panel.md` — panel architecture the view plugs into

## Completion Criteria
- [ ] Child tasks EUD-081, EUD-082 are done
- [ ] `npm --prefix panel run build` and `npm --prefix panel test` pass