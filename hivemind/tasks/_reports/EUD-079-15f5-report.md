---
task_id: EUD-079-15f5
completed_at: 2026-06-06T14:40:00
duration_minutes: 16
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 7
  spec_compliance: 9
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 480000
  output: 80000
cost_usd: 13.20
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Added the `memory_write` MCP tool and its journal integration (features/07 "MCP tool:
memory_write", decision 07): `ToolSpec("memory_write", "write", ...)` with `file` enum
(resources|structure|conventions|lessons) + `content`, first-line validation (enum,
8,192-byte UTF-8 cap, disabled/missing store → ToolError before any disk write), writes
through the injected `ProjectMemory` store (`ToolLayer(bridge, memory=...)`), plan-gate
EXEMPT (never raises PlanRequired, mutation counter untouched) while consuming the
30-action budget, `structure` writes refresh the LIST staleness hash via the new
`list_reply=` kwarg threaded through `call`/`call_for_request`. Journal: snapshot
`{content, existed}`, changeset item `{kind: memory, target: memory/<file>, diff}` with a
server-side unified diff, inverse restores old content or deletes the file when
`existed=false`, wired into the existing reverse-seq replay; accept archives normally.
`mcp_shim.py` untouched (specs fetched dynamically).

TDD protocol followed: Step A added 17 failing tests (orchestrator confirmed 17 failed /
91 passed); Step B made them pass without touching the tests.

## Changes

- `server/eud_agent/tools.py` — `MEMORY_TOOL` ToolSpec + `_h_memory_write` validator +
  `_memory_write_via_store` routing (pre-gate, post-budget), `ToolLayer(memory=...)`
  injection, `list_reply=` kwarg
- `server/eud_agent/journal.py` — `Journal(memory=...)` + `load(memory=...)`,
  `memory_write` snapshot `{content, existed}`, `memory` changeset category/item,
  `_rollback_memory` inverse (restore vs delete), `_target_for`/`after_for` branches
- `server/tests/test_tools.py` — 10 new tests (registry/schema, enum/cap/no-project
  rejection without disk writes, store write, plan-gate exemption + budget consumption,
  BudgetExceeded, structure LIST-hash refresh, return-shape JSON-serializability)
- `server/tests/test_journal.py` — 7 new tests (snapshot both cases, changeset memory
  item + unified diff, rollback restore/delete, mixed reverse-seq replay, accept archive)

## Verification

Run by the orchestrator in the worker worktree (verify.md stages lint + test):

- `python -m ruff check server` → `All checks passed!`
- `python -m pytest server/tests/test_tools.py server/tests/test_journal.py -q` →
  `108 passed`
- `python -m pytest server/tests -q` → `3 failed, 595 passed, 7 skipped` — the 3 failures
  are the known-environmental `test_deploy_scripts` cases (worker worktree has no local
  `server/.venv`); no new failures.

## Review

One BLOCKING finding, fixed in review round 1:

- `_memory_write_via_store` returned the `WriteResult` dataclass — not JSON-serializable
  at the `/tools/call` endpoint (`JSONResponse` + debug-trail `json.dumps`); latent until
  EUD-081 injects a store, then a successful end-to-end write would 500. Fixed: returns
  `{"ok": True, "file": <file>}`; a regression test now pins the shape and asserts
  `json.dumps()` succeeds.

Advisory (one fixed, one accepted):

- `_h_memory_write` triple-encoded `content` — hoisted to a single `encode` (fixed).
- journal `_read_memory`/`_rollback_memory` duplicate the store's `<name>.md` path
  convention (private `_file_path`) — accepted coupling, no clean store seam without
  scope expansion.

Rubric after fix: correctness 7 (defect was latent-but-real, now fixed + regression
test), spec_compliance 9, safety 9 (snapshot→write→record ordering leaves no stale
journal entry on write failure; missing store refuses rollback per-item), clarity 9.

## Harness Sync

- harness sync: no-op (all touched files already documented — `tools.py`/`journal.py`
  are listed in `features/05_agent-core.md ## Implementation` and the memory additions in
  `features/07_project-memory.md ## Implementation`; no manifest changes)

## Incident

### What broke
- Review found a blocking boundary defect: `memory_write` returned a `WriteResult`
  dataclass that the `/tools/call` endpoint cannot JSON-serialize (would 500 once
  EUD-081 wires a real store).

### Why
- Unit tests exercised the ToolLayer in-process and never crossed the JSON endpoint
  boundary, so a non-serializable return passed all 108 tests.

### What fixed it
- Review round 1: return `{"ok": True, "file": <file>}` (matching the `{ok, ...}` style
  of `build_run`) plus a test asserting `json.dumps()` of the return value succeeds.

## Notes

- Worker worktree again created from the stale base 23bc6f4; the worker self-rebased
  onto main per orchestrator instruction (now a standing instruction for this repo).
- Mid-task, prior-session uncommitted work was committed to main as a WIP checkpoint
  (user decision) to unblock EUD-081's scope; EUD-079's merge was unaffected (disjoint
  files, three-dot diffs used after main moved).
- Baseline on main after the WIP commit: 2 failing `test_deploy_scripts` PowerShell 5.1
  tests (execution policy `UnauthorizedAccess` — the test spawns `powershell.exe` without
  `-ExecutionPolicy Bypass`). Pre-existing, unrelated to this task.
