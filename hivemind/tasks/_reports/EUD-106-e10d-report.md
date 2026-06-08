---
task_id: EUD-106-e10d
completed_at: 2026-06-08T14:30:00
duration_minutes: 30
coding_retries: 1
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 8
  clarity: 10
tokens:
  estimated: true
  input: 125000
  output: 43000
cost_usd: 5.10
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Ported the map-write safety rails + map journal to `src-tauri/src/mapsafe.rs` (the first
agent-core Rust module beyond the isom FFI + Tauri scaffold). Every mutating map write runs,
IN ORDER: (1) compiling guard, (2) lock probe, (3) full-file backup, (4) all-or-nothing
apply, (5) re-digest verify, (6) journal entry; rollback (rail 7) restores the backed-up
bytes via a temp file + atomic rename, refusing while the map is locked. External
collaborators (compiling status, lock probe, map engine) are abstracted behind traits so the
full sequence is unit-testable with no live editor, no real map, and WITHOUT adding the
`isom` crate dependency — the real isom/bridge wiring lands in a later task.

This task also re-confirmed (empirically) that the **ort/MSVC STL link blocker (EUD-100) is
resolved**: the local toolchain is now MSVC 14.44.35207 (≥ the 14.41 ort prebuilt needs), and
a `cargo test -p eud-agent --no-run` that pulls fastembed→ort linked cleanly.

## Changes
- `src-tauri/src/mapsafe.rs` (new, +703) — `MapSafeError` (thiserror, one variant per rail),
  traits `CompilingStatus` / `LockProbe` / `MapEngine`, real dependency-free `WindowsLockProbe`
  (raw `extern "system"` `CreateFileW`/`CloseHandle`/`GetLastError`, no `windows-sys`/`winapi`
  crate), `JournalEntry { map_path, backup_path }`, and `MapSafe<S,L,E>` with `write` (rail
  sequence) + `restore` (rail 7) + `backup`. 8 unit tests (one per rail + refusal/edge paths)
  using real temp dirs and injected fakes.
- `src-tauri/src/lib.rs` (+1) — `pub mod mapsafe;`.

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- `cargo test -p eud-agent mapsafe` → **8 passed; 0 failed** (the Step-A failing tests now
  pass — verify-first gate satisfied: orchestrator independently confirmed all 8 FAILED on
  `todo!()` before implementation).
- `cargo clippy -p eud-agent --all-targets -- -D warnings` → clean.
- `cargo fmt -p eud-agent -- --check` → clean.
- Scope: only `src-tauri/src/mapsafe.rs` + `src-tauri/src/lib.rs` changed (lib.rs scope-added
  for the module declaration; disjoint from all peers — none in flight). merge-base = `main`
  (`9b8cfd2`), not stale at merge.

## Review
Reviewer (opus-4-7) found **no blocking issues**. Rubric: correctness 9, spec_compliance 9,
safety 8, clarity 10 — all above blocking thresholds. Four advisories: (1) rail-5 verify
failure left a possibly-corrupt map with no rollback handle; (2) apply-failure orphans the
backup file (harmless); (3) `backup_timestamp` nanos collision is theoretical on the
serialized write path; (4) `extern "system"` non-`unsafe` form is correct for edition 2021.

Advisory (1) — the only real safety gap in a safety-critical module — was addressed before
merge (commit `0f0d9f2`): `MapSafeError::Verify` now carries `{ detail, backup }` so a
post-write corruption is recoverable (the caller can rebuild a `JournalEntry` and `restore`);
auto-restore is intentionally NOT done (it could overwrite forensic state and can itself
fail). A test asserts the surfaced backup path holds the original pre-edit bytes. Advisories
(2)–(4) left as-is (harmless / theoretical / correct-now).

## Harness Sync
- features/13_isom-ffi.md already lists `src-tauri/src/mapsafe.rs` under `## Implementation`
  ("rails + journal (ports journal.py)") — binding is a no-op (idempotent, already in sync).
- `src-tauri/src/lib.rs` is module-declaration glue (not separately bound).
- No manifest changed. Contract-drift guard: no spec mentions of mapsafe internals
  (`MapSafeError`/`Verify`/`fn write`/`fn restore`) — no drift.

## Notes
- Behavioral source: Python `chk_info.restore_map_backup` + `windows_file_locked`, and the
  feature-09 "Safety rails (service, in order)" sequence. mapsafe is generic over the op
  buffer — location NAME bytes pass through RAW, never re-encoded (rules.md).
- Build cache: a shared `CARGO_TARGET_DIR` (`.cargo-shared-target/`, untracked) was warmed so
  worker + verification builds reuse the compiled ort/fastembed/tauri deps (avoids a ~15-min
  cold compile per worktree). Consider gitignoring or pruning it.

## Incident

### What broke
- The coding worker's isolated worktree was branched from a **stale base** (`23bc6f4`, a
  POC-era EUD-038 commit), not `main` — it had none of the v2 Rust work (`crates/isom`,
  `src-tauri/src/config.rs`). The worker's pre-flight base check caught it and STOPPED before
  Step A (coding_retries: 1 for the Step-A re-do after the base refresh).
- The Agent runtime then placed the refreshed worker in the worktree dir
  `agent-a514590355803861b`, which was the **preserved EUD-100 (blocked) worktree**; the
  `git reset --hard main` moved its branch off the EUD-100 bootstrap commit (`3f95678`).

### Why
- Known issue (memory `agent-worktree-stale-base`): the Agent tool can cut a worktree from an
  old commit. The EUD-100 worktree/branch reuse is a runtime artifact of resuming the worker.

### What fixed it
- Instructed the worker (SendMessage) to `git reset --hard main` to refresh onto `9b8cfd2`,
  then re-run Step A. merge-base verified = main before merge.
- The EUD-100 bootstrap commit `3f95678` (and `4fea4da`) survive as git objects; the
  orchestrator protected them on a named branch `eud-100-bootstrap` so the (now-unblocked)
  EUD-100 work is not lost.
