---
task_id: EUD-058-babc
completed_at: 2026-06-05T15:35:51
duration_minutes: 35
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 5
  spec_compliance: 8
  safety: 8
  clarity: 9
tokens:
  estimated: true
  input: 382416
  output: 95604
cost_usd: 12.91
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Panel WS v2 protocol + store state machine (features/06 State machine + Behaviors; protocol per features/05 WS v2). Protocol/store layer only — the v2 UI components (PlanView/ChangesetView/AgentStream) are the next task; App.tsx was reduced to a v2 placeholder shell that keeps the build green.

- **`protocol.ts`** — full v2 rewrite: client→server `chat`/`plan_feedback`/`plan_approve`/`changeset_decision{decision, ids:"all"|string[]}`/`cancel`/`status`/`list`; server→client `agent_event`/`answer`/`plan{markdown,revision}`/`changeset{request_id,items[]}`/`rollback_result{ids,ok}`/`error`/`status`/`progress`/`list`. v1 `instruct`/`apply`/`code`/`applied` REMOVED (absence-guard test). Every field name traced against engine.py/app.py emissions (reviewer re-verified each).
- **`store.ts`** — v2 state machine (`connecting→ready⇄thinking→plan_review|changeset_review`); reconnect mid-thinking → ready + notice (server cancels via the disconnect handler — assumption verified against app.py `finally → engine.aclose()`); changeset stays reviewable across reconnect (journal server-persisted); plan revision replacement; **decision-aware `rollback_result` handling** (see Review); send gating v2 `connected && hasProject && !busy` (settable-target gate REMOVED); Korean labels + 500-entry log cap retained.
- **Component fallout (scope-added)**: spec-removed `ApplyBar`/`TargetPicker`/`ReviewTabs` (+tests) deleted per features/06 "Removed"; `App.tsx`/`ConversationLog`/`InstructionBox` (+tests) minimally adapted; `MonacoEditor`/`lib/diff`/`lib/truncate` retained orphaned for the ChangesetView task.

Verify-first gate: Step A failing suite committed first (28 failed / 7 passed red against the v1 store).

## Changes

17 files: `panel/src/ws/protocol.ts` (rewrite), `client.ts` (docs), `state/store.ts` (rework), `state/store.test.ts` (40 tests), App/ConversationLog/InstructionBox + their tests, protocol/client tests migrated, 6 v1 component files deleted.

## Verification

- Step A red (28 failed / 7 passed). Worker worktree post-review: vitest **109/109** (10 files), `npm run build` green — both orchestrator re-run.
- Merged main tree: panel build green; server suite unaffected (**493 passed / 4 skipped**).

## Review

Review round 1 — initial rubric: **correctness 5 (blocking)**, spec 8, safety 8, clarity 9. The reviewer traced every protocol field against the server source (all outbound shapes exact) and found the inbound ACCEPT path broken:

- **F1 (blocking)**: `rollback_result{ids, ok}` carries NO accept/reject discriminator, and the store labeled every ok=true as "rejected" (되돌림) — a user's KEPT changes were reported as rolled back.
- **F2 (blocking)**: bulk accept (`ids:"all"`) — the server replies with `ids: []` (it does not echo accepted ids), so zero items got decided and the panel stayed in `changeset_review` forever.
- F3: the existing test false-passed (asserted only the phase, not the decision label).

Fixed (commit 8f056f5): `decisionSent` now RECORDS the pending decision (store is the sole sender; WS ordered; one decision in flight); `rollback_result` labels per the recorded decision (accept→"accepted"; reject→ok?"rejected":"failed"); recorded-"all" + empty server ids resolves against all currently-undecided items (prior per-item decisions preserved); defensive fallback for an unrecorded reply; App.tsx log line reflects the real decision (적용 유지/되돌림/일부 실패). 8 new/updated tests incl. the bulk-accept shape.

Advisory (recorded): the server protocol would ideally echo the decision (or accepted ids) in `rollback_result` — candidate engine.py amendment for a later task; the recorded-decision approach is correct while the store is the only sender.

## Incident

### What broke
- The inbound accept half of the changeset flow: accepted items mislabeled as rolled back; bulk accept stranded the panel in review.

### Why
- `rollback_result` is decision-agnostic on the wire and the engine's accept-all branch returns empty ids; the store inferred semantics from `ok` alone. The test suite asserted phases but not decision labels, so it false-passed.

### What fixed it
- Review round 1: pending-decision recording in the store + undecided-resolution for the empty-ids bulk-accept shape + label-asserting tests (vitest 104→109).

## Harness Sync

harness sync: no-op for bindings — protocol.ts/client.ts/store.ts already in features/06 `## Implementation`; deleted components match the spec's "removed" list exactly (the spec already documents the removal — no contract drift; the deletion IS the contract). No manifest dependency changes (package-lock untouched). Pass.

## Notes

- Next panel task (EUD-059?) builds the v2 UI: PlanView/ChangesetView/AgentStream/Header status visibility — the store now exposes `pendingDecision`, changeset state, and plan revisions for it.
- Server-side follow-up candidate: include the decision (or echo accepted ids) in `rollback_result` so the protocol is self-describing.
- Monaco lazy chunks are currently absent from dist (nothing imports MonacoEditor in the shell) — they return when ChangesetView wires previews.
