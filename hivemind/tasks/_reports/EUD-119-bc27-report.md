---
task_id: EUD-119-bc27
completed_at: 2026-06-09T09:05:00Z
duration_minutes: 40
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: 4874409
    output: 40636
    total: 4915045
  reviewer_tracked: false
---

## Summary
Replaced the panel's WebSocket transport with a Tauri IPC client, 1:1 with the existing
v2 chat schema (feature 15 / decision 13). Added `panel/src/lib/ipc.ts` (`IpcClient`:
`invoke` for commands chat/plan_feedback/plan_approve/changeset_decision/cancel/reset/
status/list; `listen` for push events agent_event/answer/plan/changeset/rollback_result/
progress/error/status; status+list also surfaced from their `invoke` return values).
Re-homed every shared protocol export (ClientMessage/ServerMessage unions, type guards,
FileEntry/ChangesetItem/Diagnostic/ProgressStage, PROGRESS_STAGES, *_MESSAGE_TYPES) into
`panel/src/lib/protocol.ts`, re-exported by `ipc.ts`. Rewired `state/store.ts`, `App.tsx`,
and every importer (components + lib/changeset) to `@/lib/ipc`. Deleted `panel/src/ws/*`.
Removed all WS/token/Origin/reconnect/`server.ready` code; readiness is now driven by IPC
listener registration + the initial status/list resolve (no reconnect loop). The store's
`wsConnecting`/`wsOpen`/`wsError` hooks were kept by name as transport-neutral phase
drivers.

## Changes
- new: `panel/src/lib/ipc.ts`, `panel/src/lib/protocol.ts`
- new (verify-first): `panel/src/lib/ipc.test.ts` (5 tests — injectable invoke/listen,
  command mapping, push dispatch, status/list result surfacing, readiness w/o reconnect)
- edited: `panel/src/App.tsx` (WsClient → IpcClient; async sends; `void connect()`),
  `panel/src/state/store.ts` (import path + comment retarget; hook names unchanged),
  `panel/src/state/store.test.ts`, `panel/src/components/{ChangesetView,DiagnosticsStrip,
  Header,InstructionBox,PlanView}.tsx(+test)`, `panel/src/lib/changeset.ts(+test)`
  (import-path retarget only)
- deleted: `panel/src/ws/client.ts`, `panel/src/ws/client.test.ts`,
  `panel/src/ws/protocol.ts` (moved → lib/protocol.ts), `panel/src/ws/protocol.test.ts`
- dep: `@tauri-apps/api ^2.11.0` added to `panel/package.json` (+ lock)

## Verification
- `cd panel && npx tsc -b` → exit 0 (no dangling `@/ws/*` imports).
- `cd panel && npx vitest run` → 14 files, 202 tests passed (incl. new ipc.test.ts 5/5;
  old ws/* tests removed).
- Verify-first gate confirmed: `ipc.test.ts` failed before implementation
  ("Failed to resolve import @/lib/ipc") and passes after.

## Review
codex review (`--base main`) raised one [P1]:
- "Register v2 IPC commands before invoking them" (ipc.ts:206) — the panel now invokes the
  v2 command surface (chat/plan_feedback/plan_approve/reset/changeset_decision), but the
  Rust backend on this branch (`src-tauri/src/lib.rs`) still registers only the v1
  instruct/apply/status/list commands, so the integrated app fails at runtime.

Disposition: **acknowledged known cross-task integration dependency, not a defect of this
task.** Decision 13 explicitly re-plans the backend to the v2 chat surface via SEPARATE
rebuild tasks (superseding EUD-110/113/114); feature 15 states "the Rust backend (feature
11) is rebuilt to match" the panel contract. No valid in-scope fix exists: adding Rust
commands is out of this task's panel-only scope AND would trip the contract-drift guard
against the superseded backend; reverting the panel to v1 is the alternative Decision 13
rejected. The panel deliverable is correct against its contract. Tracked by the backend
v2-rebuild tasks; not blocking the panel transport migration.

## Notes
- Sandbox interaction: the codex `workspace-write` worker could write files but could not
  (a) create `.git/.../index.lock` (worktree gitdir lives outside the workspace root) nor
  (b) spawn esbuild for vitest. The orchestrator therefore performed all git commits and
  ran all verification (tsc/vitest) directly. Worker did file edits only.
- Scope-add (no in-flight peers; disjoint): broadened scope to `panel/src/lib`,
  `panel/src/components`, `panel/package.json`, `panel/package-lock.json` for the
  type-rehoming import updates inherent to the task (original scope under-declared
  `panel/src/lib/ipc.ts` alone).
- Contract-drift guard: PASS — protocol identifiers were MOVED (ws → lib) exactly as
  feature 15 mandates, not removed/renamed (git detected the file rename at 69%); no
  signature change; no rules.md-contradicting comment added.

## Harness Sync
- features/15_panel-tauri-ipc.md += `panel/src/lib/protocol.ts` (BOUND, commit 51bfdae)
- tech-stack.md ## Active Dependencies: `@tauri-apps/api ^2` already present — idempotent
  (no-op); installed version resolved to ^2.11.0.
- components + lib/changeset already documented under features 03/06 — no new binding.
