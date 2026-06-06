---
task_id: EUD-081-cd4a
completed_at: 2026-06-06T17:45:00
duration_minutes: 95
coding_retries: 1
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 8
  clarity: 9
tokens:
  estimated: true
  input: 720000
  output: 130000
cost_usd: 20.55
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Added the memory WS surface and completed the end-to-end project-memory wiring
(features/07 "WS protocol additions"):

- `memory_get {}` → `memory {project, files{resources,structure,conventions,lessons},
  episodes}` (last 50, newest FIRST — store order reversed); no project → `error`.
- `memory_save {file, content}` → direct `ProjectMemory.write` (NOT journaled — user
  edits are not agent mutations); `memory_saved {file}` / `error` (unknown file,
  oversize, no project; surfaces `WriteResult.reason`). All IO via `asyncio.to_thread`.
- Handled in app.py's `_serve_ws` BEFORE `engine.handle()` (documented deviation:
  status/list route through the engine, but engine dispatch was out of the original
  scope; protocol-level behavior matches the spec).
- Wiring: `AgentEngine(..., data_dir=cfg.data_dir)`; `_LiveProjectMemory` proxy
  (a `__getattr__` forwarder that resolves a fresh `ProjectMemory` from live bridge
  STATUS per access) injected as `ToolLayer(memory=)` and `Journal(memory=)` so the
  project name resolves at use time (no boot-time caching; mid-session switch honored).
- **Engine finalization-ordering fix (scope-add approved)**: `state = "idle"` moved
  BEFORE the episode-record await in `_finish_turn` (answer-only) and in
  `_on_changeset_decision._decide()` (success path; except branch also sets idle).
  `_finalize_prior_request` audited race-free (documented in-code). Root cause: an
  EUD-080 latent defect — the episode `asyncio.to_thread` yields the event loop while
  state was still `executing`/`changeset_review`, so the client's next message raced
  into the busy guard once `data_dir` was actually wired. Proven by git-stash bisect;
  two new spy tests pin the ordering (reviewer revert-verified both fail without the
  reorders).

TDD protocol followed: Step A added 9 failing tests (orchestrator-confirmed
9 failed / 30 passed, 4.5s, no hang); Step B (app.py) made them pass; the engine fix
round added 2 ordering-guard tests.

## Changes

- `server/eud_agent/app.py` — memory_get/memory_save handlers, `_resolve_memory`,
  `_LiveProjectMemory` proxy, engine `data_dir` + ToolLayer/Journal memory wiring
- `server/eud_agent/engine.py` — finalization ordering: idle-before-episode at two
  sites + audit comment at the third
- `server/tests/test_app.py` — 6 new WS endpoint tests
- `server/tests/test_integration_ws.py` — 2 new end-to-end tests (journaled
  memory_write → changeset memory item → reject → disk content restored;
  memory_save → memory_get round-trip) + hang-proof receive helpers; coverage gate
  extended with `memory`/`memory_saved`
- `server/tests/test_agent_flow.py` — 2 spy tests pinning state==idle at
  `_record_episode` call time

## Verification

Run by the orchestrator in the worker worktree:

- ruff (5 changed files) → `All checks passed!`
- `pytest tests/test_agent_flow.py tests/test_app.py tests/test_integration_ws.py
  tests/test_tools.py tests/test_journal.py tests/test_memory.py -q` →
  `248 passed, 1 skipped` (6.06s, no hang)
- `pytest tests --ignore=tests/test_rag.py -q` → `2 failed, 618 passed, 6 skipped` —
  only the environmental PowerShell-5.1 deploy baseline
- `pytest tests/test_rag.py -q` → `18 passed, 1 skipped` (worker's offline-hang concern
  did not reproduce; rag unaffected)

## Review

No blocking findings; rubric 9/9/8/9. The reviewer independently applied the diff and
revert-checked the two spy tests (both fail without the engine reorder — genuine
guards). Advisory findings recorded:

1. **F1 (follow-up recommended before live E2E)**: `_LiveProjectMemory.__getattr__`
   re-resolves via `bridge.status()` per ATTRIBUTE access — one journaled
   `memory_write` costs ~4-5 STATUS file-IPC round-trips (~4-5s on the real 1s-tick
   bridge; invisible under the instant fake bridge). Per-operation resolution would
   collapse to 1 round-trip. The engine's own episode path already resolves once.
2. **F2**: project switch between two attribute accesses yields a torn operation
   (snapshot from A, write to B) — unreachable under the single-user single-instance
   topology, eliminated for free by the F1 fix; proxy docstring should note the hazard.
3. **F3**: memory replies go to the requesting socket only (not broadcast) — matches
   the spec's refresh-on-memory_get model; intentional.

## Harness Sync

- harness sync: no-op (app.py/engine.py already bound in `features/05_agent-core.md
  ## Implementation`; memory surface documented in `features/07_project-memory.md`;
  no manifest changes)

## Incident

### What broke
1. Step A's first delivery hung instead of failing: three pytest runs blocked ~1 hour
   (worker's run, orchestrator's verification run, and a retry) and the worker ended
   its turn without committing, waiting on a notification that never comes.
2. Step B's mandatory `data_dir` wiring surfaced an EUD-080 latent race (3
   test_agent_flow failures): episode recording awaited before `state = "idle"`.

### Why
1. The server replies `error {unknown message type}` once to unhandled WS types; the
   tests drained unboundedly for a `memory` reply — starlette's TestClient
   `receive_json()` has no timeout and pytest-timeout is not installed, so the second
   receive blocked forever.
2. With `data_dir=None` (all of EUD-080's tests), `_record_episode` returned at its
   first line with no event-loop yield — the race was structurally invisible until a
   later task wired the seam.

### What fixed it
1. Coding retry 1: hang-proof receive helpers (`_recv_expecting` = exactly one
   receive; `_recv_turn_end` = hard-bounded drain treating `error` as terminal).
2. Orchestrator-approved scope-add (engine.py + test_agent_flow.py, no in-flight
   peers): idle-before-episode reorder at both racy sites + spy regression tests.

## Notes

- Stale worktree base again (23bc6f4); worker self-rebased per standing instruction.
- F1 (proxy STATUS round-trip multiplication) is the one item worth a small follow-up
  task before the live-editor E2E; it is a quality issue, not a correctness one.
- EUD-082 (panel memory view) deliberately deferred by user decision (2026-06-06):
  the server-side memory loop is complete; the panel renders `memory/<file>` changeset
  items generically until EUD-082 lands.
