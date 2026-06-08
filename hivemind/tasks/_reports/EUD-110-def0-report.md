---
task_id: EUD-110-def0
completed_at: 2026-06-08T08:04:53Z
duration_minutes: 35
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
  input: 1529286
  output: 25788
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: 019ea63d-88f8-7063-9202-96aab9fdf7c7
  coder_tokens:
    input: 1529286
    output: 25788
    total: 1555074
  reviewer_tracked: false
---

## Summary
Defined the Tauri IPC surface for the eud-agent Rust core in `src-tauri/src/ipc.rs`,
replacing the old localhost WebSocket protocol with idiomatic Tauri `invoke` commands +
emitted events (Decision 11). Four commands (`instruct`, `apply`, `status`, `list`) are
registered with the Tauri builder, and typed event payloads (`progress`, `code`,
`agent_event`, `applied`, `error`) plus typed emit helpers are provided for the engine
task to use. No localhost socket, token, Origin check, or `server.ready` was introduced.
Command/event payload types carry serde derives whose wire format (camelCase `useContext`,
enum strings `set`/`neweps`, stage strings, `eps` lang, `pct` omitted when None) matches
the documented schema; command handlers are thin compiling placeholders (emit `error` +
return `Err(ENGINE_NOT_WIRED…)`) until the engine orchestration task wires them.

## Changes
- `src-tauri/src/ipc.rs` (new) — serde payload types for all commands/events, four
  `#[tauri::command]` handlers, and per-event typed `emit_*` helpers + `#[cfg(test)]`
  wire-schema tests.
- `src-tauri/src/lib.rs` — `pub mod ipc;` + `.invoke_handler(generate_handler![ipc::instruct,
  ipc::apply, ipc::status, ipc::list])` on the builder chain; updated the shell doc comment.

## Verification
Run in the worker worktree against the shared cargo target cache.
- `cargo test -p eud-agent ipc` — 7 passed, 0 failed (6 ipc wire-schema tests + 1 matched
  config test). Verify-first gate confirmed the artifact failed to compile (26 errors)
  before implementation.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.

Note: the worktree lacked the gitignored `panel/dist` build artifact, which
`tauri::generate_context!` requires at compile time; the built artifact was copied from the
main checkout into the worktree to allow headless Rust verification (not a code change, not
committed).

## Review
Codex review (`codex review --base main`) returned one blocking finding:
- [P1] instruct/apply used a single struct param (`request: InstructRequest`), which makes
  Tauri expect `invoke("instruct", { request: {...} })`, contradicting the documented flat
  panel contract `invoke("instruct", { instruction, target, useContext })`.

Resolved in review round 1: handlers now take the payload fields as separate parameters
(Tauri 2 maps JS camelCase arg keys to Rust snake_case), so the flat invoke envelope
matches the documented schema. The request structs are kept (canonical typed payloads +
schema tests) and reconstructed inside each handler to avoid drift. Re-verification
(test/fmt/clippy) passed.

## Notes
- Scope was extended with `hv task scope-add EUD-110-def0 src-tauri/src/lib.rs`: registering
  commands with the Tauri builder (a completion criterion) is only possible in `lib.rs`.
  Sequential mode, no in-flight peers — disjointness trivially held.
- Harness sync: no-op — both touched files were already documented under a feature
  `## Implementation` section (`ipc.rs` in feature 11, `lib.rs` in feature 10); no manifest
  change. Contract-drift guard: clean (removed lines were doc-comment text only).
- Codex worker could not commit from the workspace-write sandbox (the worktree git metadata
  lives under the main repo `.git/worktrees/…`, outside the writable root). The orchestrator
  committed each step's working-tree changes on the worker's behalf.
- `cost_usd` is 0.00 because both providers are Codex; the claude pricing table does not
  cover `gpt-5.2-codex`. Codex coder token counts are recorded under `codex_usage`;
  reviewer tokens are not tracked (Codex review path).

## Incident

### What broke
- Codex review flagged a [P1] blocking issue: the `instruct`/`apply` Tauri commands took a
  single struct parameter, so Tauri's argument deserialization would require a wrapped
  `{ request: {...} }` invoke envelope — incompatible with the documented flat panel
  contract `{ instruction, target, useContext }` / `{ mode, target, code }`.

### Why
- Tauri derives each command's JS argument key from the Rust parameter name. A single
  `request: T` parameter therefore nests the payload under `request`, whereas the IPC
  contract this task defines requires the payload fields at the top level of the invoke
  args (1:1 with the retired WebSocket message shape).

### What fixed it
- On review round 1, the handler signatures were changed to accept the payload fields as
  separate parameters (`instruction, target, use_context` / `mode, target, code`); the
  typed request structs are reconstructed inside the handler bodies. test/fmt/clippy
  re-verified clean.
