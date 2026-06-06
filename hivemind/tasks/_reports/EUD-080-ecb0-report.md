---
task_id: EUD-080-ecb0
completed_at: 2026-06-06T15:20:00
duration_minutes: 14
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 285000
  output: 48000
cost_usd: 7.88
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Wired project memory into the engine (features/07 "Prompt injection" + "Episodes"):

- `build_system_prompt(..., data_dir=None)` inserts the `[project memory]` section
  (rendered by `ProjectMemory.render_section()`) BETWEEN `[first principles]` and
  `[reference context]`. The project name is reused from the SAME STATUS fetch that
  builds `[project state]` (new `_fetch_project_state()` returns section + name; no
  second bridge round-trip).
- `AgentEngine(..., data_dir=None)`; `_resume_turn_text()` prepends a refreshed
  `[project memory]` alongside the refreshed `[project state]` + `[reference context]`;
  project name re-resolved per turn (mid-session project switch follows on next chat).
- Episode recording at every finalization point via `ProjectMemory.append_episode`:
  `_on_changeset_decision` → accepted/rejected/partial, default-accept of prior
  undecided items → defaulted, answer-only turn end → answer. Shape
  `{ts ISO8601, request_id, instruction ≤200 chars (retained chat text), kind,
  tools distinct, files distinct, decision}`. Only when a project name is known; all
  errors logged + swallowed (memory never breaks the request flow); IO off the event
  loop via `asyncio.to_thread`.
- `data_dir=None` default preserves every existing caller — app.py wires it in EUD-081.

TDD protocol followed: Step A added 8 failing tests (orchestrator confirmed 8 failed /
42 passed); Step B implemented in engine.py only, no test edits.

## Changes

- `server/eud_agent/engine.py` — `_fetch_project_state`, `_project_memory_section`,
  `_journal_tools_and_files`, `data_dir` seams, `_chat_text` retention,
  `_record_episode`/`_append_episode`, finalization wiring
- `server/tests/test_agent_flow.py` — 8 new tests (section order pinned by index
  comparison, resume refresh, no-project degradation, one-episode-per-path counts +
  exact shape, data_dir-omitted backward compat)

## Verification

Run by the orchestrator in the worker worktree:

- `python -m ruff check server/eud_agent/engine.py server/tests/test_agent_flow.py` →
  `All checks passed!` (note: `ruff check server` on main itself has 3 pre-existing
  errors in debuglog.py / test_debuglog.py / test_deploy_scripts.py from the WIP
  checkpoint commit — outside this task's scope)
- `python -m pytest server/tests/test_agent_flow.py -q` → `50 passed, 1 skipped`
- `python -m pytest server/tests -q` → `2 failed, 626 passed, 7 skipped` — the 2
  failures are the main-baseline PowerShell 5.1 deploy tests (execution policy), not
  regressions.

## Review

No blocking findings; rubric 9/9/10/9. Advisory findings recorded:

1. An `apply`-kind turn whose changeset is empty (nothing journaled) records no
   episode — narrow gap vs "one episode per finalization"; consider a spec note.
2. `partial` is recorded at most once per changeset (the engine goes idle after any
   single decision — pre-existing behavior), and a subset-ids decision covering all
   items is still labeled `partial`. Low impact (over-warns codex at worst).
3. `render_section()` is called WITHOUT `list_reply`, so prompts never carry the
   staleness suffix: `bridge.list_files()` consumes the raw LIST reply internally, and
   threading it out is a bridge-API change out of scope. The suffix still works via the
   `memory_write(structure)` hash refresh; consider a one-line spec note.
4. `_append_episode` issues a fresh STATUS round-trip per finalization (correct for the
   project-switch edge, runs off-loop); on the `defaulted` path this is awaited before a
   new chat starts — worst case ~1s added latency over the 1s-tick file IPC.

## Harness Sync

- harness sync: no-op (engine.py already bound in `features/05_agent-core.md
  ## Implementation`; memory wiring documented in `features/07_project-memory.md`;
  no manifest changes)

## Notes

- Worker worktree again created from the stale base 23bc6f4; worker self-rebased onto
  main per its standing first-action instruction (pattern: every worktree this session).
- Reviewer test-quality gap noted for a future pass: the `defaulted` episode's
  instruction (prior-request text retention timing) is not directly asserted by a test.
