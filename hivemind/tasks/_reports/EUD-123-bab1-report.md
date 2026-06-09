---
task_id: EUD-123-bab1
completed_at: 2026-06-09T11:14:10
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
  input: 891894
  output: 51088
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019eaa08-9559-7e51-a493-2a874384ebc7
  coder_tokens:
    input: 891894
    output: 51088
    total: 942982
  reviewer_tracked: false
---

## Summary

Replaced the single-shot `codex exec` agentic drive with a codex **app-server JSON-RPC over
stdio** client in `src-tauri/src/codex_client.rs` (decision 13). The new
`CodexAppServerClient<R, W>` is generic over injected `AsyncRead`/`AsyncWrite`, so the
transport is unit-tested against an in-process stub server (tokio `duplex`) with no real
codex binary. It drives the thread lifecycle (`initialize` -> `thread/start` -> `turn/start`,
resume via `thread/resume` reusing the captured thread id — EUD-064 continuity), forwards
streamed notifications as a typed `AppServerEvent` stream over an mpsc channel, and answers
server approval requests: ACCEPT for the eud-tools `mcpServer/elicitation/request`, DECLINE
for `item/commandExecution/requestApproval` / `item/fileChange/requestApproval` (and legacy
`execCommandApproval`/`applyPatchApproval`). A `spawn_app_server` helper resolves codex via
`resolve_codex_cmd()` and launches `codex app-server` with the measured `-c` overrides
(`skills.include_instructions=false`, `model_supports_reasoning_summaries=true`,
`model_reasoning_summary="detailed"`) over piped stdio with `kill_on_drop`. The existing
single-shot `extract_code`/`build_prompt`/`generate` path and its tests are retained intact
(scope-limited task; the spec retires it for the agentic flow but does not delete it here).

## Changes

- `src-tauri/src/codex_client.rs` (+830): `AppServerEvent` enum, `AppServerError`,
  `CodexAppServerClient<R, W>` (`new_with_stdio`, `run_turn`, request/response correlation
  via oneshot map, monotonic ids), background stdout reader dispatching request/notification/
  response by JSON-RPC shape, method-specific approval replies, `spawn_app_server` production
  constructor, and a `mod appserver_tests` stub-server integration test.

## Verification

- `cargo test --workspace` — 72 passed, 1 ignored, 0 failed (includes the new
  `appserver_tests::app_server_json_rpc_stdio_streaming_thread_reuse_and_approvals`).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — clean (orchestrator ran
  `cargo fmt` to normalize; the sandboxed worker cannot run rustfmt).
- Verification-first gate: the stub-server test was added first and confirmed FAILING
  (runtime `todo!()` panic at codex_client.rs:376) before implementation, then made to pass.

## Review

Codex review (`codex review --base main`) raised 3×P1 + 1×P2, ALL confirmed against the real
codex 0.137 app-server schema (verified by the orchestrator via `codex app-server
generate-ts`) and fixed in one review round:

- [P1] `initialize` requires `clientInfo {name, version}` (was empty `{}`) — fixed to send
  `clientInfo` with `env!("CARGO_PKG_VERSION")`.
- [P1] `thread/started` carries the id at `params.thread.id`, not top-level `threadId` — the
  parser (and the stub, which had encoded the wrong shape) were corrected; otherwise
  `await_thread_started` would hang forever against a real server.
- [P1] `turn/start` requires `threadId` + `input: [{type:"text", text, text_elements:[]}]`,
  not a top-level `prompt` — fixed.
- [P2] approval decline decisions are method-specific (`decision:"decline"` for the v2
  command/file-change requests, `decision:"denied"` for legacy `ReviewDecision`) — fixed with
  a per-method match.

After the fix: full re-verification green (tests + clippy + fmt).

## Harness Sync

- no-op: the only touched file `src-tauri/src/codex_client.rs` is already documented under
  `features/11_rust-backend-core.md` ## Implementation; no manifest changed. Contract-drift
  guard: no spec-promised identifier removed/renamed (insertions only; existing single-shot
  API kept).

## Notes

- The active profile `mixed` names `gpt-5.2-codex` as executor/reviewer, but that model is
  **not supported on a ChatGPT (BYO) account** ("model is not supported when using Codex with
  a ChatGPT account", HTTP 400). The worker therefore ran on the codex default `gpt-5.5`. The
  profile model id should be reconciled with what the account actually supports.
- Cost is recorded as 0.00 because the Codex coder runs on a BYO ChatGPT account (no
  per-token billing in the claude pricing map). Coder token counts are summed per-turn across
  the resumed session (includes re-sent/cached context on each resume).
- Spec-vs-reality grounding: the `codex app-server quirks` memory documents the OLD Python
  SDK 0.1.0b3 protocol. The installed codex CLI is 0.137; the orchestrator generated the live
  TS bindings and confirmed the spec-described method names (`thread/start`, `thread/resume`,
  `turn/start`, `mcpServer/elicitation/request`, shell/patch approval requests, reasoning
  deltas) all still exist, while the param/response SCHEMAS differ — which is exactly what the
  review caught and fixed.

## Incident

### What broke
- Verification: a stray extra closing brace at codex_client.rs:813 from the Step B worker
  caused `error: unexpected closing delimiter` — the crate did not compile (verify_retries=1).
- Review: the initial implementation invented JSON-RPC param/response shapes that did not
  match the real codex 0.137 app-server schema (3×P1 + 1×P2). The stub test had encoded the
  same wrong shapes, so it passed without exercising the real protocol.

### Why
- The brace was a mechanical edit slip when moving helper functions to module scope.
- The protocol-shape divergence: the worker had no access to the live app-server schema and
  the spec/memory describe the older SDK protocol at the method-name level, not the v0.137
  param/response field layout.

### What fixed it
- Brace: one resume round removing the duplicate `}` (re-verified compiling).
- Protocol shapes: the orchestrator generated the authoritative schema via `codex app-server
  generate-ts`, extracted the exact `InitializeParams`/`ThreadStartedNotification`/
  `TurnStartParams`/approval-decision enums, and the worker corrected BOTH the implementation
  and the stub to the real shapes in one review round.
