---
task_id: EUD-126-0e23
completed_at: 2026-06-09T00:00:00Z
duration_minutes: 95
coding_retries: 1
verify_retries: 1
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: codex
  reviewer: codex
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
  coder_session_id: 019eaaad-63a6-7fc1-a4f8-743cea3e96e9
  coder_tokens:
    input: 1210358
    output: 9506
    total: 1219864
  reviewer_tracked: false
---

## Summary
Rebuilt `src-tauri/src/engine.rs` into the agentic turn-loop state machine
(`idle -> triage -> answer | plan_review* -> executing -> changeset_review -> idle`) and wired
it into the Tauri app shell (`src-tauri/src/lib.rs`), per decision 13. The retired v1
single-shot instruct seam (`run_instruct` / `InstructOutput` / `EngineError` / pub `unified_diff`
/ `normalize_generated_code`) was removed; the pure prompt assembly seam
(`build_system_prompt` / `resume_turn_text` + section helpers) was preserved.

The engine is generic over two crate-private seams — `CodexDriver` (turn execution) and
`EventSink` (event emission) — so the state machine is unit-testable with a fake driver and a
capturing sink. Production impls wrap `codex_client::CodexAppServerClient` and the Tauri
`AppHandle` (`ipc::emit_*`). The six v2 commands (`chat`, `plan_feedback`, `plan_approve`,
`changeset_decision`, `cancel`, `reset`) are exposed as engine `#[tauri::command]` handlers
(via `rename = "..."` so the wire names match the panel) and registered in `lib.rs`; `ipc.rs`
placeholder commands are left untouched (out of scope).

Behavior wired: first `chat` builds the system prompt; every subsequent `chat` resumes with
`resume_turn_text` (refreshed `[project state]` + memory + `[reference context]` then a
`[user message]` header — EUD-092); `reset` drops the retained thread (EUD-064); each `chat`
mints a fresh per-request id; plan gating reuses `tools::RequestState`; journaled changesets
are emitted via `journal::JournalStore`.

## Changes
- `src-tauri/src/engine.rs` — agentic state machine (`AgentEngine<D, S>`, `Phase`,
  `CodexTurnResult`, `AgentEngineError`, `CodexDriver`, `EventSink`, `EngineEvent`,
  `AgentEngineConfig`), production `ProductionCodexDriver` + `TauriEventSink`, the six
  `#[tauri::command]` handlers, journal/changeset mapping helpers; removed the single-shot
  instruct seam and its tests; kept the prompt-assembly helpers + their tests.
- `src-tauri/src/lib.rs` — `.setup()` constructs the engine and `.manage()`s it behind a
  `tokio::sync::Mutex`; `invoke_handler` registers the engine commands (keeping `ipc::status`
  / `ipc::list`).

## Verification
All run by the orchestrator from the worker worktree against the shared target cache
(`CARGO_TARGET_DIR=.cargo-shared-target`), with `panel/dist` copied in so
`generate_context!()` resolves (build artifact only — gitignored, present on `main`):
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean (exit 0).
- `cargo test --workspace` — 90 passed / 0 failed (lib) + all other crates green, incl. the
  two new engine state-machine tests
  (`agentic_engine_chat_uses_system_prompt_then_resume_prompt_then_reset_system_prompt`,
  `agentic_engine_routes_answer_only_and_propose_plan_turns_to_v2_events`).
- `cargo build --manifest-path src-tauri/Cargo.toml` — exit 0 (cdylib links ONNX Runtime;
  `generate_context!()` resolves).

Completion criteria:
- [PASS] state machine routes chat/plan_feedback/plan_approve/changeset_decision/cancel/reset
  to v2 events — six handlers in engine.rs, registered in lib.rs.
- [PASS] thread start-vs-resume + reset (EUD-064); `[user message]` labeling on resume
  (EUD-092) — verified by the new engine test.
- [PASS] single-shot instruct seam removed; `cargo test --workspace` +
  `cargo clippy --workspace --all-targets -- -D warnings` pass.

## Review
Codex review (`codex review --base main`) returned 4 findings: 2× [P1], 2× [P2] (blocking).
One fix round was applied (by the orchestrator — see Incident for why) and re-verified green:
- [P2] streaming event kinds — FIXED: emit `agent_event` kind `reasoning` (was
  `reasoning_delta`) and stream answer deltas as kind `delta` while still accumulating, so the
  panel's live reasoning/answer surfaces update mid-turn.
- [P2] finalize pending changeset across turns — FIXED: `chat` and `reset` now call
  `journal::JournalStore::finalize_undecided_as_accepted` for the prior request when a
  changeset is still under review (EUD-070 default-accept).
- [P1] per-item accept archived the whole journal — FIXED: a partial accept (`accept` with a
  specific id list) no longer archives the journal; remaining items stay pending and
  rejectable. Only accept-all and rejects finalize the journal.
- [P1] reject rollback is non-functional (`UnsupportedJournalBridge` errors on every inverse
  op) — DOCUMENTED as a follow-up, NOT fixed: a real `JournalBridge` needs editor file-IPC
  inverse ops that `bridge_io` does not yet expose (delete/rename/set_main/plugin commands)
  plus a live editor connection — both outside this task's 2-file scope and the current
  `bridge_io` surface. A clarifying comment marks the placeholder.

## Notes
- The distributed `panel/dist` is gitignored and absent in a fresh worktree; it was copied
  from `main` for the build only. It is NOT part of the merge.
- Scope-drift gate: only `engine.rs` + `lib.rs` were modified — exactly the declared scope.
- Harness sync: no-op — both files are already bound under `features/11_rust-backend-core.md`
  `## Implementation`; no manifest changed. Contract-drift guard: the removed
  `run_instruct`/`InstructOutput` identifiers appear in `features/11` only as explicitly
  "retired (decision 13)", so removal ratifies the spec rather than drifting from it.
- Follow-up: wire a `bridge_io`-backed `JournalBridge` so changeset `reject` actually rolls
  back (requires extending the bridge command set with the inverse ops).

## Incident

### What broke
- The Codex worker could not self-verify or commit: its `workspace-write` sandbox blocks
  network (so `ort-sys` cannot download ONNX Runtime → cdylib link fails), starves MSBuild for
  the path-sensitive `isom-sys` rebuild (CLR exit `0xE0434352` under page-file pressure), and
  is read-only over the worktree's git metadata in the parent repo (`index.lock` denied).
- `codex exec resume` rejects the `-C`/`-s` flags that `codex exec` accepts, so the Step B
  resume invocation failed (exit 2).
- The Step A failing test asserted `!prompt.contains("[user message]")` for the first turn, but
  `build_system_prompt` legitimately contains that substring in its `[message format]` section.
- The worker added an out-of-scope `generate_context!(assets = empty)` workaround in `lib.rs`
  for the missing `panel/dist`.

### Why
- The sandbox model is fundamentally incompatible with this project's verification (network +
  memory + cross-worktree git), so the delegate-then-verify loop could not converge inside the
  worker. Root cause is environmental, not the worker's code.

### What fixed it
- The orchestrator (unsandboxed, networked) ran ALL verification against the shared cache,
  committed on the worker's behalf, and re-dispatched Step B as a fresh `codex exec` (since
  resume rejects `-C`/`-s`). The worker still authored the full engine implementation.
- Orchestrator corrections (all in-scope, all re-verified green): tightened the Step A
  assertion from a substring check to a line-exact `[user message]` header check (the real
  resume marker); reverted the unrelated `generate_context!` change to the original surgical
  form; applied the three in-scope review fixes above.
