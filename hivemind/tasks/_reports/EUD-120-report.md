---
task_id: EUD-120-ecca
completed_at: 2026-06-10T11:00:00Z
duration_minutes: 30
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: false
  input: 3584404
  output: 47725
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019eaf26-cd3c-79b3-92c5-c36703afc525
  coder_tokens:
    input: 3584404
    output: 47725
    total: 3632129
  reviewer_tracked: false
---

## Summary
Added the first-run setup screen and editor-connection-state UI to the React panel:
- A full-screen `SetupScreen` overlay rendered while bootstrap is active, showing
  download progress (determinate progress bar from the `pct` field, indeterminate when
  absent) and an error mode with a retry control.
- An editor-not-connected `ConnectionNotice` banner shown when the store reports a
  stale/absent bridge heartbeat; `instruct`/`apply` are disabled because the new
  `editorConnected` flag is folded into the store's `canSend` gate, and the
  `InstructionBox` placeholder explains why.

Model override: the active `mixed` profile names `gpt-5.2-codex`, which this ChatGPT
account rejects with HTTP 400; both coder and reviewer ran on `gpt-5.5` (the verified
invocation, per the codex-cli-orchestrator-invocation lesson).

## Changes
New:
- `panel/src/setup/bootstrap.ts` — pure `bootstrapView(pct, detail)` mapping the
  `{stage:"bootstrap", pct, detail}` progress payload to setup view state.
- `panel/src/setup/SetupScreen.tsx` — first-run overlay (role="dialog", progressbar, error+retry).
- `panel/src/components/ConnectionNotice.tsx` — editor-not-connected banner (role="status").
Edited:
- `panel/src/state/store.ts` — `editorConnected` field (default true, fail-open) folded into
  `canSend`; `EDITOR_DISCONNECTED_MARKER`; `applyStatus` sets it true, `errorReceived` flips it
  false on the marker; new `editorConnectionChanged` action.
- `panel/src/App.tsx` — routes `progress {stage:"bootstrap"}` to the setup overlay; dismisses on
  the first non-bootstrap progress (rag_warmup → app moved past bootstrap); mounts
  `ConnectionNotice`; retry reloads.
- `panel/src/components/InstructionBox.tsx` — editor-not-connected placeholder hint.
- `panel/src/lib/protocol.ts` — added optional `pct?: number` to `ProgressMessage` (matches the
  Rust emitter + feature 10 `progress {stage:bootstrap, detail, pct}`).
Tests (failing-first, then green):
- `panel/src/setup/bootstrap.test.ts`, `panel/src/setup/SetupScreen.test.tsx`,
  `panel/src/components/ConnectionNotice.test.tsx`, `panel/src/state/store.editor.test.ts`.

## Verification
- `cd panel && npx tsc -b --noEmit` → clean (exit 0).
- `cd panel && npx vitest run` → 21 files / 237 tests passed (19 new).
- Verify-first gate confirmed: the four new test files failed before implementation
  (6 failing assertions, incl. `store.editorConnectionChanged is not a function`).
- Rust stages (cargo fmt/clippy/test) not run — the change is panel-only; no `src-tauri/`
  or `crates/` files were touched.

## Review
Codex review (read-only, `--base main`) returned two blocking findings, both valid against the
existing Rust emitter in `src-tauri/src/bootstrap.rs`:
- [P1] The panel parsed `pct` out of `detail` and dismissed on `detail==="done"`, but the emitter
  sends `pct` as a separate field and never emits a "done" sentinel (pct=100 fires once per asset).
  A successful install would leave the overlay up forever with an always-indeterminate bar. Fixed:
  consume the `pct` field; dismiss when the first non-bootstrap progress (rag_warmup) arrives,
  which strictly follows bootstrap per feature 10's flow.
- [P2] The retry button invoked `status` (reads editor state only), which does not re-run the
  first-run install. Fixed: retry now reloads (feature 10's documented recovery is "resume on next
  launch"); a dedicated in-process bootstrap-retry IPC command remains future backend work.
Both addressed in one review round; re-verified green.

## Notes
- `pct` was added to `ProgressMessage` to align the panel type with the backend's actual
  emission (`src-tauri/src/bootstrap.rs` already sends it; feature 10 documents it). This is a
  binding/sync addition (optional field), not a contract change — no spec identifier was
  removed/renamed.
- Bootstrap-complete handoff: there is no distinct "bootstrap done" event from the backend
  (bootstrap_assets is not yet wired into `lib.rs`); the panel keys dismissal on the first
  non-bootstrap progress (rag_warmup) which is the architecture's post-bootstrap init signal.
- In-app retry re-download is a backend gap (no bootstrap-retry IPC command in the v2 contract);
  the panel reload is the faithful interim affordance.
- E2E (editor live; bootstrap real download) is user-assisted/GUI — not headless; static
  verification (tsc + vitest) covers the criteria.

## Incident

### What broke
- Code review flagged two blocking contract mismatches between the panel's bootstrap UI and the
  existing Rust bootstrap emitter: pct parsed from `detail` instead of the dedicated field, and a
  non-existent `detail:"done"` completion sentinel; plus a retry wired to the wrong command.

### Why
- The panel design was grounded only in `panel/src/lib/protocol.ts` (where `ProgressMessage` had
  no `pct`) and feature 10's prose, without cross-checking the realized emitter in
  `src-tauri/src/bootstrap.rs`, which is the actual contract.

### What fixed it
- On the single review round: added `pct?` to `ProgressMessage`, switched to `bootstrapView(pct,
  detail)` consuming the field, dismissed the overlay on the first non-bootstrap progress, and
  changed retry to a reload. tsc + 237 tests green afterward.
