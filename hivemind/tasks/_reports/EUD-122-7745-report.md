---
task_id: EUD-122-7745
completed_at: 2026-06-09T10:40:00Z
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
    input: 1071218
    output: 21572
    total: 1092790
  reviewer_tracked: false
---

## Summary
Replaced the v1 instruct/apply/code/applied Tauri IPC surface in `src-tauri/src/ipc.rs`
with the v2 chat schema (decision 13) and registered it in `src-tauri/src/lib.rs`.
Command bodies are thin placeholders (engine wiring lands in EUD-126); this task owns the
surface + serde contract. serde payloads match the panel v2 protocol (`panel/src/lib/
protocol.ts`, moved there by EUD-119).

Commands: chat{text}, plan_feedback{text}, plan_approve{}, changeset_decision{decision,ids},
cancel{}, reset{}, status{}→{compiling,project}, list{}→{files[]}. Events (emit helpers):
agent_event{kind,detail,data?}, answer{text}, plan{markdown,revision}, changeset{request_id,
items[]}, rollback_result{ids,ok}, progress{stage,detail?}, error{message}, status (push).
`changeset_decision.ids` is an untagged `DecisionIds` = `"all"` | `string[]`, hardened so
only the exact literal `"all"` deserializes to the bulk variant (review fix, below).
`ChangesetItem` flattens arbitrary extra fields; `AgentEvent.data` and `progress.detail`
are skip-if-none.

## Changes
- `src-tauri/src/ipc.rs` — v1 types/handlers/events removed (InstructRequest, ApplyRequest,
  ApplyMode, ApplyResponse, CodeEvent, CodeLang, AppliedEvent, instruct/apply, emit_code/
  emit_applied, ENGINE_NOT_WIRED); v2 types + thin handlers + emit helpers added; test
  module rewritten to v2 wire round-trips (8 tests incl. verify-first + the P2 rejection).
- `src-tauri/src/lib.rs` — `generate_handler!` registers the 8 v2 commands (instruct/apply
  removed).

## Verification
- `cargo test ipc` (CARGO_TARGET_DIR=shared, panel/dist copied for generate_context) → 7
  ipc tests pass (serde round-trips for every command/event; "all"/list round-trip; non-"all"
  string rejection). Verify-first gate confirmed: the v2 test failed to compile (24 errors —
  undefined v2 types) before implementation, passes after.
- `cargo clippy --workspace --all-targets -- -D warnings` → clean.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean (orchestrator
  normalized a wrapped-line diff with `cargo fmt`).

## Review
codex review (`--base main`) raised one [P2]:
- "Reject non-all string changeset ids" (ipc.rs DecisionIds) — the untagged `All(String)`
  variant accepted ANY bare string, so a single id mistakenly sent as a string (not an
  array) would deserialize as the bulk `All` case and could accept/reject every pending
  item once the handler is wired.

Resolution (1 review round): added an `AllLiteral` marker type whose `Deserialize` succeeds
ONLY for the exact string `"all"` (and `Serialize` emits `"all"`); `DecisionIds::All` now
wraps `AllLiteral`. A non-"all" bare string fails the `All` arm and—being a non-array—also
fails `List`, yielding a deserialize error instead of a silent bulk decision. Added a test
asserting `"a"`/`""`/`"All"` and `ids:"nope"` are rejected. Re-verified: cargo test ipc 7/7,
clippy clean.

## Notes
- Sandbox: codex `workspace-write` worker could not run git/cargo/clippy/rustc (worktree
  gitdir index.lock + process-spawn denials); the orchestrator did all commits, ran
  `cargo test ipc` / clippy / fmt via the shared `CARGO_TARGET_DIR`, and copied `panel/dist`
  into the worktree so `tauri::generate_context!()` compiles.
- Contract: removing v1 instruct/apply is spec-MANDATED by decision 13 (the v1 surface is
  superseded), not contract drift. The new command/event surface matches feature 11 exactly.
- This resolves the integration gap flagged by EUD-119's review [P1] (panel invoked v2
  commands the backend did not yet expose); the backend now exposes the v2 command surface
  (handlers remain thin until EUD-126 wires the engine).

## Harness Sync
- Skipped (no-op): both touched source files (`src-tauri/src/ipc.rs`, `src-tauri/src/lib.rs`)
  are already listed under features/11_rust-backend-core.md ## Implementation, and no
  manifest changed. No binding append needed.

## Incident

### What broke
- codex review [P2]: `DecisionIds::All(String)` (untagged serde) accepted any bare string,
  not just the contract literal `"all"` — a single id sent as a string would misroute to a
  bulk accept/reject.

### Why
- An untagged serde enum variant of type `String` matches ANY string. Serde untagged tries
  variants in order and accepts the first that deserializes; `All(String)` swallowed every
  string before `List(Vec<String>)` could be considered, with no literal validation.

### What fixed it
- Introduced an `AllLiteral` marker type with a custom `Deserialize` that errors unless the
  input is exactly `"all"` (and `Serialize` emits `"all"`); `DecisionIds::All` wraps it.
  Non-"all" strings now produce a deserialize error. Fixed on the single allowed review
  round (commit 9ac3f14).
