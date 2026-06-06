---
created: '2026-06-06'
depends_on:
- EUD-078-3b29
id: EUD-081-cd4a
parent: EUD-077-de1e
priority: medium
scope:
- server/eud_agent/app.py
- server/tests/test_app.py
- server/tests/test_integration_ws.py
status: pending
title: memory_get/memory_save WS endpoints + integration test
type: task
updated: '2026-06-06'
---

## Description
Add the memory WS surface to `app.py` (spec "WS protocol additions").

- `memory_get {}` → `memory {project, files: {resources, structure, conventions, lessons},
  episodes: [...]}` (last 50 episodes, newest first). No project open → `error {message}`.
- `memory_save {file, content}` → direct ProjectMemory write (NOT journaled — user edits
  are not agent mutations), same file enum + 8 KB cap; reply `memory_saved {file}` or
  `error {message}`.
- Route through the engine's `handle()` dispatch like `status`/`list`; file IO off the
  event loop (`asyncio.to_thread`, matching the journal decision pattern).
- Extend the fake-bridge WS integration test: chat → `memory_write` → changeset `memory`
  item → reject → `memory_get` shows restored content; plus a `memory_save` round-trip.

## Spec References
- [[features/07_project-memory|07_project-memory]] `../docs/features/07_project-memory.md` — WS protocol additions, edge cases
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` — WS protocol v2, engine dispatch

## Completion Criteria
- [ ] `memory_get` returns the four files + episodes for an open project and `error` when none is open
- [ ] `memory_save` rejects unknown file / oversize content with `error` and persists valid content (verified by a following `memory_get`)
- [ ] Integration test passes: reject path restores pre-`memory_write` content end-to-end over WS
- [ ] `ruff check .` passes; `pytest -q tests/test_app.py tests/test_integration_ws.py` passes