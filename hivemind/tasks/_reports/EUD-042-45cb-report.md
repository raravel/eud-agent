---
task_id: EUD-042-45cb
completed_at: 2026-06-05T13:17:12
duration_minutes: 20
coding_retries: 1
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 10
  spec_compliance: 10
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 179245
  output: 44811
cost_usd: 6.05
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Fixed the zombie-server leak on quick editor restart: the server's only shutdown path was heartbeat staleness, but a restarted editor resumes writing the SAME `heartbeat.txt`, so superseded servers never exited (each holding bge-m3 GPU memory and racing the new server for `srv-*` IPC files; observed live 2026-06-05).

The 15s `HeartbeatWatcher` loop now ALSO reads `server.ready` each tick (supersede check FIRST, before staleness): a parseable ready file with a token DIFFERENT from this process's own → a newer server owns the data dir → log + self-terminate WITHOUT deleting `server.ready` (it belongs to the new server). Missing/unreadable/corrupt/token-less ready = NO decision (transient states; staleness remains the fallback). Token comparison (not pid) per EUD-037. The 60s staleness threshold and all existing heartbeat behavior are unchanged (user decision: EUD-039's measured 54s UI stall forbids shorter thresholds).

The skip extends through the REAL exit path: `HeartbeatWatcher.superseded` is set before `on_stale()`, and the FastAPI lifespan shutdown hook delegates to `_shutdown_cleanup(watcher, delete_ready)` which always stops the watcher but deletes ready ONLY when not superseded.

## Changes

- `server/eud_agent/app.py` — `HeartbeatWatcher.own_token` + `superseded` attributes; `_is_superseded()` (UTF-8 no-BOM read, JSON parse, token compare, every ambiguity = no decision); supersede check before staleness in `_run`; `_shutdown_cleanup()` extracted (testable) and wired into the lifespan shutdown hook; `_on_stale` callback no longer deletes ready (deletion ownership consolidated in the watcher's staleness path + the guarded shutdown hook).
- `server/tests/test_app.py` (+221) — 4 acceptance tests (different token → fire + ready preserved through the full shutdown path; own token / missing / corrupt → no exit) + graceful-exit guard test (non-superseded shutdown still deletes) ; heartbeat-refresher pattern isolates the supersede decision from staleness; generous timing (no flakiness).

## Verification

Verify-first gate: Step A committed the 4 acceptance tests first; orchestrator-confirmed failing (4 failed — `TypeError: unexpected keyword argument 'own_token'`) against the unfixed watcher.

- Worker worktree: ruff clean; pytest 316 passed / 4 skipped.
- Merged main tree: ruff clean; **317 passed / 3 skipped**.

## Review

Verdict: approve. Rubric: correctness 10, spec_compliance 10, safety 9, clarity 9. No blocking findings.

Key review result — the quick-restart race timeline is airtight: the new server's startup deletes any pre-existing ready synchronously BEFORE `watcher.start()`, and the watcher waits a full interval before its first check, so the new server can never observe the old token; the old server's ready-writer is one-shot, so its next tick after the new ready lands sees the differing token and exits. No both-exit / neither-exit / wrong-delete interleaving. `superseded` bool sharing is safe via the `should_exit` happens-before chain (documented in-code).

Advisory (recorded): rules.md "Delete it [server.ready] on graceful shutdown" wording predates the supersede exception — a one-line amendment would stop a future reader from "fixing" the skip. Attempted via `hv feedback save --target rules`; rejected by the misfiring BM25 dedup gate (matched the unrelated `never-bypass-hvtask-...` L2 doc again, score 4.74). Skipped per the no-workaround rule — the amendment text is preserved here for manual application.

## Incident

### What broke
- The first fix skipped ready-deletion in the watcher and the `_on_stale` callback, but the REAL production exit path still clobbered the new server's ready: supersede → `should_exit` → uvicorn exits → FastAPI `@app.on_event("shutdown")` → unconditional `_delete_ready()`.

### Why
- The unit tests exercised the watcher + callback directly and never drove the lifespan shutdown event, so they passed while the deployed flow was still broken. Found by the orchestrator tracing the full exit path through `create_app`'s lifecycle hooks.

### What fixed it
- Retry 1: `watcher.superseded` flag (set before `on_stale`) + `_shutdown_cleanup(watcher, delete_ready)` extraction — the lifespan hook now skips deletion exactly on the superseded path; a new guard test pins the graceful-exit direction too (deletion must still happen when not superseded).

## Harness Sync

harness sync: no-op (all touched files already documented) — `app.py` is listed in features/02 `## Implementation`; test file is a test; no manifest changes. Contract-drift guard: no spec-promised identifiers removed (internal callback refactor only). Pass.

## Notes

- rules.md follow-up wording (manual, when convenient): append to the "ALWAYS self-terminate when heartbeat.txt is stale" rule — "EXCEPTION: when exiting because `server.ready` carries another server's token (superseded, EUD-042), the dying server must NOT delete `server.ready` — it belongs to the new server."
- The `hv feedback save` BM25 dedup gate rejected every non-binding lesson this session against the same unrelated L2 doc (`never-bypass-hvtask-agent-team-pipeline-always-use-model-pro-2.md`, scores 4.7-7.6) — needs hv-side calibration.
