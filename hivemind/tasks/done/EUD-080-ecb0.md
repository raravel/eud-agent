---
completed_at: '2026-06-06T14:45:23.497606'
created: '2026-06-06'
depends_on:
- EUD-079-15f5
id: EUD-080-ecb0
parent: EUD-076-d2c2
priority: high
scope:
- server/eud_agent/engine.py
- server/tests/test_agent_flow.py
status: done
title: '[project memory] prompt injection + episode recording at finalization'
type: task
updated: '2026-06-06'
---

## Description
Wire project memory into the engine (spec "Prompt injection", "Episodes").

- `build_system_prompt()`: insert the rendered `[project memory]` section BETWEEN
  `[first principles]` and `[reference context]`.
- `_resume_turn_text()`: include a refreshed `[project memory]` copy alongside the
  refreshed `[project state]` + `[reference context]` (memory changes between chats).
- Resolve the project name per turn from the STATUS already fetched for `[project state]`;
  project switch mid-session follows the new project on the next chat.
- Episode recording at every finalization point: `_on_changeset_decision`
  (accepted/rejected/partial), default-accept on next chat (defaulted), answer-only turn
  end (answer). Line shape per spec: ts, request_id, instruction head (200 chars), kind,
  distinct tools, distinct files, decision. Only when a project name is known; failures
  logged and swallowed.

## Spec References
- [[features/07_project-memory|07_project-memory]] `../docs/features/07_project-memory.md` — Prompt injection, Episodes
- [[features/05_agent-core|05_agent-core]] `../docs/features/05_agent-core.md` — system prompt assembly, EUD-064 resume contract, finalization paths

## Completion Criteria
- [ ] First-chat system prompt contains `[project memory]` between `[first principles]` and `[reference context]`; resumed turn text contains a refreshed copy
- [ ] No project open → section renders `(no project memory)` and the turn proceeds (RAG-style degradation)
- [ ] Each finalization path (accept / reject / partial / defaulted / answer-only) appends exactly one correctly-shaped episode line
- [ ] `ruff check .` passes; `pytest -q tests/test_agent_flow.py` passes