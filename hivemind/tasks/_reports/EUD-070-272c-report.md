---
task_id: EUD-070-272c
completed_at: 2026-06-05T19:55:00
duration_minutes: 25
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 45000
  output: 10000
cost_usd: 1.80
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-direct
---

## Summary

Live-E2E defect 5 ("되돌리기나 적용을 하면 동기적 렉이 발생함"). Two causes:

- A rollback replays inverse ops SEQUENTIALLY over the 1s-tick file IPC (the S1
  changeset was one dat group with 3 properties → 3 SETDAT round-trips ≈ 2-4s),
  and `_serve_ws` awaited `engine.handle(...)` INLINE — the WS receive loop was
  blocked for the whole rollback, so every other click (including a fast accept)
  queued behind it.
- The panel locked all controls via `pendingDecision` with ZERO progress UI, so
  even the intended wait read as a freeze.

**Fix**: `_on_changeset_decision` validates synchronously (state guard, journal
lookup, unknown-decision error) then runs the journal work as a BACKGROUND task
(`self._decision_task`, mirroring `_turn_task`): the receive loop stays free;
the state leaves `changeset_review` only when the task completes. Guards: one
decision at a time (second → "busy: a decision is already in flight"); a `chat`
arriving mid-decision DRAINS the decision first (no journal race); `aclose`
cancels the decision task like the turn task. Panel: ChangesetView shows a
spinner notice (결정 처리 중… (되돌리기는 에디터에 한 건씩 적용됩니다)) while
`pending`.

## Changes

`server/eud_agent/engine.py`, `server/tests/test_orchestrator.py` (+3 tests
with a blocking journal: responsiveness, double-decision error, chat-drains-
decision; 2 existing tests updated to drain the background task),
`panel/src/components/ChangesetView.tsx` (+2 tests), features/05 doc.

## Verification

Verify-first: the responsiveness test red (rollback_result emitted inline);
green after — the orchestrator suite dropped from 30.1s (blocking-journal
timeouts hit the inline path) to 0.05s. Full suites: server 528 passed / ruff
clean / selfcheck OK; panel vitest 215 passed / build green.

## Harness Sync

features/05 gained the "Background changeset decisions" contract paragraph. No
manifest changes.

## Notes

- The per-op 1s bridge-tick latency itself is physics (file IPC on the editor
  UI-thread tick) — not eliminated, but now visible (spinner notice) and
  non-blocking (other messages process during it).
