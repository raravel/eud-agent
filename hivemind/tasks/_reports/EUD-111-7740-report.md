---
task_id: EUD-111-7740
completed_at: 2026-06-08T08:32:50Z
duration_minutes: 28
coding_retries: 0
verify_retries: 1
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
  input: 5452445
  output: 56650
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: 019ea647-03e6-7ab0-b114-b9ff5ae6f31a
  coder_tokens:
    input: 5452445
    output: 56650
    total: 5509095
  reviewer_tracked: false
---

## Summary
Ported the v1 Python file-IPC client (`server/eud_agent/bridge_io.py`) to a new Rust module
`src-tauri/src/bridge_io.rs`. The client writes `srv-<id8>.cmd` into the editor's
`Data\agent\inbox` as raw UTF-8 **without BOM** (temp + atomic rename, byte-exact, no
`\n`->`\r\n` translation) and polls `outbox\srv-<id8>.result`. The poll deadline extends
from the default 10s `timeout` to the 180s `busy_timeout` once `status.txt` reports
`compiling=true` (including builds that start mid-wait), firing the `on_busy` callback exactly
once (the orchestrator forwards `waiting_build`). On timeout the `.cmd` is left in place;
`ERROR:`-prefixed replies become `BridgeError::Error`; the reader deletes the `.result` after
consuming. Thin wrappers cover the feature-11 command set (`ping`/`status`/`list`/`get`/`set`/
`neweps`/`getdat`/`setdat`/`build`/`lua`) with the v1 GET/SET/NEWEPS line shapes, the LIST
`path\t<EFileType>` parse + CUI/RawText settable derivation, and GETDAT/SETDAT dat-name/
non-negative/numeric validation before send. The features/04 extended surface
(xdat/tbl/req/btn/file-tree/settings/plugins) is intentionally out of scope (a later tools
task). `cleanup_stale` clears only the `srv-*` namespace, never legacy `agent_*` files. ID
generation is std-only (a process-local `AtomicU64` counter seeded from `SystemTime` nanos),
so no new crate was added (Cargo.toml was out of scope).

## Changes
- `src-tauri/src/bridge_io.rs` (new) — `BridgeIo`, `SendOpts`, `BridgeError`, `FileEntry`,
  `send` with the two-poll result-stabilization guard, command wrappers, validation helpers,
  `cleanup_stale`, std-only `id8`, and `#[cfg(test)]` tests.
- `src-tauri/src/lib.rs` — `pub mod bridge_io;` declaration.

## Verification
Run in the worker worktree against the shared cargo target cache (the gitignored
`panel/dist` build artifact was copied in so `tauri::generate_context!` compiles headlessly).
Verify-first gate confirmed the artifact failed to compile (unresolved `bridge_io::` types)
before implementation.
- `cargo test -p eud-agent bridge_io` — 5 passed, 0 failed (round-trip vs fake bridge, busy
  timeout + once-only on_busy + .cmd retained, cleanup namespace, no-BOM byte-exact, and a
  deterministic result-stabilization test).
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — exit 0.
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.

## Review
Codex review (`codex review --base main`) returned one blocking finding:
- [P2] consume_result deleted/returned a `.result` on the first non-empty read. The real Lua
  bridge writes `.result` non-atomically (`File.WriteAllText` straight to the final path), so
  a poll landing mid-write of a large GET/LIST/LUA reply could return truncated output.

Resolved in review round 1 by extending the existing two-poll empty-read guard with a
byte-length stability guard for non-empty replies: bytes are read raw and only consumed +
UTF-8-decoded once the byte length is unchanged across two consecutive polls. This also
removes a latent mid-write UTF-8 split-read panic. The `.result` is not rewritten by the
reader (the bridge owns that file). A deterministic test for the stabilization path was added.

## Notes
- Scope was extended with `hv task scope-add EUD-111-7740 src-tauri/src/lib.rs`: the
  `pub mod bridge_io;` declaration is only possible in `lib.rs`. Sequential mode, no in-flight
  peers — disjointness trivially held.
- Harness sync: no-op — `bridge_io.rs` (feature 11) and `lib.rs` (feature 10) are already
  documented under `## Implementation`; no manifest change. Contract-drift guard: clean.
- The Codex worker could not commit from the workspace-write sandbox (worktree git metadata
  lives under the main repo `.git/worktrees/…`, outside the writable root); the orchestrator
  committed each step on its behalf.
- `cost_usd` is 0.00 — both providers are `gpt-5.2-codex`, not in the claude pricing table;
  Codex coder tokens are recorded under `codex_usage`, reviewer tokens are not tracked.

## Incident

### What broke
- Verification (clippy round): `cargo clippy -- -D warnings` failed with
  `clippy::type_complexity` on the test helper signature `Arc<Mutex<Vec<(String, Vec<u8>)>>>`.
- Review (round 1): a [P2] blocking finding — `consume_result` could return a truncated,
  non-empty `.result` if a poll landed mid-write of a large reply (the real bridge writes the
  result file non-atomically).

### Why
- type_complexity: a deeply nested generic was used inline as a function parameter type.
- truncated read: the port faithfully reproduced v1's guard, which only debounced the
  zero-length (empty) read; it did not debounce a non-empty but not-yet-fully-flushed read.

### What fixed it
- type_complexity: introduced a `type SeenLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;` alias in
  the test module (verify retry 1).
- truncated read: extended the poll state to track the last non-empty byte length and consume
  only when it is stable across two consecutive polls, decoding UTF-8 after stabilization
  (review round 1).
