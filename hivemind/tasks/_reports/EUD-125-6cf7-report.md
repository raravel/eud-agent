---
task_id: EUD-125-6cf7
completed_at: 2026-06-09T12:47:00
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
  coder_session_id: 019eaa66-640c-71e3-a829-cb135f70ac8f
  coder_tokens:
    input: 3329647
    output: 45855
    total: 3375502
  reviewer_tracked: false
---

## Summary
New `src-tauri/src/journal.rs`: a self-contained change-journal module covering per-write BEFORE
snapshots, per-request JSON persistence, reviewable changeset emission, and inverse-op rollback —
unit-testable against a fake bridge trait (no dependency on bridge_io's not-yet-implemented write
commands).

Implemented:
- `Snapshot` enum + `JournalEntry {id, seq, tool, target, before, after, ts}` capturing the BEFORE
  state per tool kind (dat/xdat/tbl/req/btn old value + was_default; file_write old content;
  file_create/mkdir created marker; file_delete full content + position; rename/move old path;
  set_main old path; settings/plugin old value/Texts/index).
- `Journal`/`JournalStore` request-scoped accumulation (ordered by seq), persistence to
  `<data_dir>/journal/<request-id>.json` as UTF-8 with NO BOM (serde_json::to_vec + fs::write of raw
  bytes), `load`, and `archive` (move to `journal/accepted/`).
- `changeset()`: dat/xdat grouped per (table, objId) with property/old/new; file items kind
  created|modified|deleted, with a `similar`-crate unified diff for modified content.
- `decide()`: `Reject` applies inverse ops through the `JournalBridge` trait in REVERSE seq order;
  `Accept` archives; mixed per-item decisions; `finalize_undecided_as_accepted`; a one-decision-at-a-
  time guard (`begin_decision` → `DecisionInProgress` on a second concurrent decision) modeled with
  an Arc<Mutex<HashSet>>.

## Changes
- `src-tauri/src/journal.rs` (NEW, +1920 incl. tests): full module.
- `src-tauri/src/lib.rs` (+1): `pub mod journal;` in alphabetical position (scope-added — the module
  must be declared for its tests to compile/run).

## Verification
Run by the orchestrator in the worker worktree against the warm shared cargo cache
(`CARGO_TARGET_DIR=.cargo-shared-target`; `panel/dist` copied in for the Tauri context macro):
- `cargo test --manifest-path src-tauri/Cargo.toml` → `test result: ok. 91 passed; 0 failed; 1 ignored`
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` → clean (exit 0)
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean (exit 0)
- Completion criteria: [PASS] snapshot→rollback round-trip per tool kind (reverse-seq inverse ops vs
  fake bridge); [PASS] changeset grouping per objId + unified diff for modified, reverse-seq rollback,
  accept archives; [PASS] journal JSON UTF-8 no BOM (first byte != 0xEF) + cargo test/clippy.

## Review
Codex review (`codex review --base 10d12e0`) raised 2 blocking findings, both judged valid and fixed
in one review round:
- [P1] Partial reject could clobber later accepted edits on the same target (rejecting a non-tail
  entry restored its `before` over a still-accepted later change). Fixed: `decide(Reject(Items))`
  now pre-validates and returns a new `JournalError::NonTailReject` BEFORE applying any inverse op
  when a rejected entry has a higher-seq, non-rejected entry on the same target (all-or-nothing).
- [P2] `file_changeset_item` called `entry_path(entry)?` eagerly, so any journal containing a
  Settings/Plugin write failed to render a changeset (those targets have no path). Fixed: the path is
  now computed lazily inside the `FileWrite` arm only; Settings/Plugin entries render `Modified`
  items without a path.
Regression tests added for both; re-verification (91 passed / clippy / fmt) confirmed.

## Harness Sync
- no-op: both touched files (`src-tauri/src/journal.rs`, `src-tauri/src/lib.rs`) are already
  documented in `features/11_rust-backend-core.md ## Implementation`; no manifest file changed; the
  diff is purely additive (new module + one module-declaration line — no contract drift).

## Notes
- Same Codex-worker Windows sandbox limitations as EUD-124 (cannot write the shared cargo cache,
  parent `.git/worktrees/.../index.lock`, so commits stay orchestrator-side; `panel/dist` copied in
  for `generate_context!`). `codex exec resume --last` (worktree as cwd) was used for Step B and the
  review-fix round to keep the same session; it does not accept `-C`/`-s` (inherited from the session)
  but accepts `-c` overrides for `writable_roots`.
- `journal.rs` defines its own `JournalBridge` trait rather than depending on `bridge_io::BridgeIo`,
  because the editor write commands the inverse ops need (RESETDAT/DELFILE/RENAME/SETMAIN/SETSET/PLUG)
  are not yet implemented in bridge_io (future tasks). Wiring the journal to the real bridge + IPC
  `changeset`/`changeset_decision` events is a later task.

## Incident

### What broke
- Code review found 2 blocking findings: partial (per-item) reject of a non-tail entry on a
  multi-edit target silently clobbered later accepted edits (P1); and changeset rendering errored for
  any journal containing a Settings/Plugin write because the file-item path was required eagerly (P2).

### Why
- The first cut applied each rejected entry's inverse independently in reverse seq without checking
  whether a later entry on the same target remained accepted, and extracted the file path before
  branching on tool kind, so path-less Setting/Plugin targets hit `InvalidEntry`.

### What fixed it
- One review round: added `JournalError::NonTailReject` with all-or-nothing pre-validation so a
  non-tail partial reject errors before any bridge op, and moved `entry_path` into the `FileWrite`
  changeset arm so Settings/Plugin items render without a path. Two regression tests added; all 91
  tests + clippy + fmt pass.
