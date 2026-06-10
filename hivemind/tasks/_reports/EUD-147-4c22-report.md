---
task_id: EUD-147-4c22
completed_at: 2026-06-10T21:29:16+09:00
duration_minutes: 20
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
  input: 78000
  output: 9000
cost_usd: 1.85
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: null
    output: null
    total: null
  reviewer_tracked: false
---

## Summary
Wired the placeholder `status` and `list` Tauri commands to real file-IPC round-trips
through the existing `bridge_io` client. `invoke("status")` now reports the editor's actual
`{compiling, project}` parsed from `status.txt`; `invoke("list")` returns the real bridge
LIST table mapped to `{path, ftype, settable}`. The editor install path is resolved from
`config.json` per call; an unset path, absent editor, or stale/absent heartbeat returns the
friendly "editor not connected" error (no panic, bounded wait).

Note on provenance: a prior interrupted run of this task had already passed the
verification-first gate (committed failing artifact `d8f948f` referencing the target seams
`read_status_snapshot_at` / `list_connected_at` / `bridge_from_config`, which fail to compile
without the implementation) and left a complete but uncommitted Step B implementation in the
worker worktree. The Codex coding session from that run was not resumable (session id not
persisted across orchestrator runs), so the orchestrator verified the recovered output
directly rather than re-spawning a fresh session. All verification was re-run from scratch.

## Changes
- `src-tauri/src/bridge_io.rs` — added `StatusSnapshot`, `BridgeError::EditorNotConnected`
  (Display = "editor not connected"), `read_status_snapshot[_at]` (parses `compiling`/`project`
  from `status.txt` after a heartbeat-freshness check), `list_connected[_at]`, the
  `ensure_heartbeat_fresh_at` helper, `HEARTBEAT_STALE_AFTER` (3s), and a `heartbeat.txt` path
  on `BridgeIo`. Existing send/poll/cleanup paths untouched (import-then-extend).
- `src-tauri/src/ipc.rs` — `BridgeManaged` state, `bridge_from_config` (resolves editor path
  from `config.json`; unset → "editor not connected"), real `status`/`list` command bodies via
  `spawn_blocking`. `list` takes an `AppHandle` and emits `progress {stage: waiting_build}` from
  its `on_busy` hook during the busy-timeout extension (review fix — see below).
- `src-tauri/src/lib.rs` — resolve `DataDirs` in setup, `cleanup_stale()` on the configured
  bridge, and `app.manage(BridgeManaged)`.

## Verification
Run by the orchestrator in the worker worktree against the shared cargo target cache:
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean.
- `cargo clippy --workspace --all-targets -- -D warnings` → no warnings.
- `cargo test --workspace` → all pass; `cargo test -p eud-agent --lib` → 137 passed, 1 ignored.
  New fake-bridge tests covering the criteria:
  `status_snapshot_reads_status_txt_when_heartbeat_is_fresh`,
  `status_snapshot_rejects_stale_or_absent_heartbeat`,
  `connected_list_round_trip_derives_settable_from_file_type`,
  `connected_list_returns_editor_not_connected_without_heartbeat_or_data_dir`,
  `bridge_state_from_config_rejects_unset_editor_path`.
- Panel stages (`tsc -b`, `vitest`, `vite build`) not affected — no panel files touched.

Completion criteria: all [PASS] — status parse, list round-trip, config-resolved editor path +
"editor not connected" contract, `.result` deletion + startup stale cleanup preserved,
fake-bridge tests, and fmt/clippy/test green.

## Review
Codex review (`codex review --base main`, default model gpt-5.5) returned two findings:

- **[P2] Emit waiting-build progress from LIST waits** (ipc.rs list) — ACCEPTED & FIXED.
  rules.md ("IPC and encoding") mandates emitting `progress {stage: waiting_build}` when the
  poll timeout extends to 180s under `compiling=true`. The initial wiring passed `None` for
  `on_busy`, dropping that signal (a `list` during a build could sit up to 180s silently). Fix:
  `list` now takes an `AppHandle` and its `on_busy` closure emits `waiting_build`. Re-verified
  (fmt/clippy/test green).

- **[P1] Panel treats editor-not-connected rejection as a fatal transport error** (claimed at
  ipc.rs) — JUDGED NON-BLOCKING FOR THIS TASK, DEFERRED. The factual claim was verified true
  against `panel/src/lib/ipc.ts`: `IpcClient.connect()` awaits `Promise.all([invoke("status"),
  invoke("list")])` and any rejection runs the catch → `stop()` (tears down listeners) with no
  reconnect loop. However, this task's explicit contract (completion criterion #3 + feature 11
  "commands return a friendly error") REQUIRES `status`/`list` to RETURN the "editor not
  connected" error; making them return `Ok` would violate the task contract (contract-drift) and
  the panel (`panel/src/lib/ipc.ts`) is out of this task's declared scope. The real gap is a
  panel-side missing editor-not-connected banner + reconnect, which belongs to feature 15
  (panel-tauri-ipc), not here. Recommend a follow-up task: panel `IpcClient` should treat a
  rejected initial `status`/`list` as the editor-not-connected state (render banner) and
  poll/retry so it recovers when the editor starts.

## Notes
- Profile `mixed` declares executor/reviewer `gpt-5.2-codex`, but the local `~/.codex/config.toml`
  default model is `gpt-5.5`; the review ran on gpt-5.5, avoiding the known gpt-5.2-codex 400 on
  ChatGPT-auth accounts.
- `src-tauri/Cargo.toml` shows a working-tree modification (CRLF/LF normalization only, no content
  change); pre-existing and unrelated — left unstaged, not part of this task's commit.
- Harness sync: no-op — `ipc.rs`, `bridge_io.rs`, `lib.rs` already listed under feature 11
  `## Implementation`; no manifest change; no contract drift.
- Worktree cleanup partial: the worker branch `hv-worker/EUD-147-4c22` was deleted and
  `git worktree prune` ran (git no longer tracks it), but the physical directory
  `C:\Users\ifthe\proj\eud\eud-agent-worker-EUD-147-4c22` could NOT be removed — a process is
  holding a handle on `native\isom\StormLib`. Not force-removed (per pipeline policy). The
  directory is a plain folder (not a junction/reparse point — safe to delete). The user should
  delete it once the locking process (likely rust-analyzer/clangd/file-watcher) releases it:
  `Remove-Item -Recurse -Force <path>`.

## Incident

### What broke
- Codex review flagged a blocking [P2]: the `list` command passed `None` for the bridge
  `on_busy` hook, so no `progress {stage: waiting_build}` event is emitted while the editor is
  compiling (up to a 180s silent wait).
- Codex review also flagged [P1]: the panel's `IpcClient.connect()` treats a rejected initial
  `status`/`list` (the editor-not-connected case) as a fatal transport failure and tears down
  without reconnect.

### Why
- P2: the initial wiring focused on the happy-path round-trip and the editor-not-connected
  error contract, and omitted the rules.md-mandated `waiting_build` emission on the busy-timeout
  extension path for `list`.
- P1: pre-existing panel behavior — `connect()` has no editor-not-connected branch and no
  reconnect loop. Out of this task's scope (panel files) and contract (the command MUST return
  the error).

### What fixed it
- P2: gave `list` an `AppHandle` and an `on_busy` closure that emits
  `progress {stage: waiting_build}`; re-ran fmt/clippy/test (green). One review round.
- P1: not changed in this task — judged a contract-respecting, out-of-scope panel concern;
  deferred to a feature-15 follow-up task (documented above).
