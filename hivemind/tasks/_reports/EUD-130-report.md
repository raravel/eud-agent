---
task_id: EUD-130-b4eb
completed_at: 2026-06-10T11:35:00Z
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
  input: 6308915
  output: 79497
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019eaf46-cccd-7cc2-b497-c55a67dd0841
  coder_tokens:
    input: 6308915
    output: 79497
    total: 6388412
  reviewer_tracked: false
---

## Summary
Implemented the `location_write` write-tool handler (the schema was registered by EUD-124; the
body was missing). Agent-driven MRGN location CRUD on the connected source map, applied through
the existing mapsafe rails + isom FFI (EUD-128), with journal/changeset integration.

- `parse_location_write` validates add/set/rename/delete BEFORE any write: per-action required
  fields, name charset (`|`/newline ban), id ≥ 1, #64 (Anywhere) refusal, and tile-rect sanity.
- `encode_locedit_ops` renders the pipe-separated locedit ops in PIXELS (tile×32) with per-axis
  inversion swap; `encode_location_name` follows the map's string-table encoding (ASCII as-is,
  STRx → UTF-8, else cp949 via `encoding_rs::EUC_KR`).
- `location_write_apply` writes via `MapSafe::write(OpKind::Locedit)` (compiling/lock/backup/
  apply/verify rails), re-digests for the result, computes the assigned id for `add`, and records
  a journaled, reviewable changeset item. On post-save verify failure it restores the map from the
  backup so the map is never left corrupt.
- journal.rs gained `WriteTool::LocationWrite`, `JournalTarget::Map`, `Snapshot::MapBackup`/
  `MapEdit`, the changeset-item mapping (Created/Deleted/Modified + map summary/path properties),
  the tail-reject target, and the `JournalBridge::restore_map_backup` rollback seam.

Model override: the `mixed` profile names `gpt-5.2-codex` (HTTP 400 on this ChatGPT account);
coder + reviewer ran on `gpt-5.5` per the verified invocation.

## Changes
- `src-tauri/src/tools.rs` — `LocWrite`, `parse_location_write`, `encode_locedit_ops`,
  `encode_location_name`, `location_write_error`, `location_write_apply`, `location_write`,
  + headless tests.
- `src-tauri/src/journal.rs` — LocationWrite journal variants + changeset/reject/rollback arms +
  `JournalBridge::restore_map_backup` + FakeBridge recording + tests.
- `src-tauri/src/engine.rs` — `UnsupportedJournalBridge::restore_map_backup` stub.

## Verification
- `cargo test -p eud-agent` → 124 passed, 1 ignored (the real-exe ffi_smoke), 0 failed. New
  passing tests: parse accept/reject ladders, ops encoding (px + inversion), name encoding
  (ascii/STRx/cp949), apply records-journal + post-edit digest, compiling-guard refusal, and
  verify-failure backup restore; journal changeset-kind + rollback-restore dispatch.
- `cargo clippy -p eud-agent --all-targets -- -D warnings` → clean.
- `cargo fmt -- --check` → clean.
- Verify-first gate confirmed: the Step A tests failed to compile (missing
  `parse_location_write`/`WriteTool::LocationWrite`/`JournalBridge::restore_map_backup`/…).

## Review
Codex review (read-only, `--base main`) returned two blocking findings, both valid:
- [P2] On `MapSafeError::Verify`, the save already happened and the backup pointer was being
  collapsed into a plain tool error with no journal entry — a corrupt map with no recovery path.
  Fixed: the Verify case restores the map from the backup (mapsafe intentionally does not
  auto-restore; the caller must) and returns a clear message; nothing is journaled because the map
  is reverted. New test asserts the on-disk bytes are restored and no entry recorded.
- [P2] The LocationWrite changeset item dropped the `JournalTarget::Map` path/summary, leaving the
  review UI unable to show which map/location. Fixed: the item now carries `summary` + `map` path
  as properties. Test asserts the summary property is present.
Both addressed in one review round; re-verified green.

## Notes
- Gates (mutation/budget/evidence) are enforced upstream in `admit_tool_call` (unchanged); the
  handler runs post-admission, so "mutation gate + budget honored" holds without handler-side code.
- No production tool dispatcher records journal entries yet (the codex tool-execution loop is a
  later wiring task), and the production `JournalBridge` is still `UnsupportedJournalBridge`
  (rollback stubbed). All journal/rollback behavior here is exercised headless via FakeBridge +
  the mapsafe FakeEngine/FakeStatus/FakeLock doubles, matching feature 09's verification plan.
- `seq`/entry-id for the journal entry derive from the current changeset item count (`loc-{seq}`);
  acceptable while location entries are 1:1 with changeset items. A shared seq source across all
  write tools is future work when the dispatcher lands.
- Real-exe E2E (locedit on a real .scx, Korean name rendering in SCMDraft) is user-assisted/GUI —
  not headless; static verification covers the criteria.

## Incident

### What broke
- Code review flagged two safety/review gaps: (1) a post-save verify failure left the map
  potentially corrupt and unrecoverable (backup pointer discarded, no journal entry); (2) the
  location changeset item carried no map path/summary, so it was not meaningfully reviewable.

### Why
- The first pass treated every `MapSafeError` uniformly and built the changeset item from only the
  entry id/kind, dropping the `JournalTarget::Map` payload — overlooking that `Verify` happens
  AFTER the on-disk save and that the review row needs the target to be useful.

### What fixed it
- Special-cased `MapSafeError::Verify` to restore from the attached backup; populated the
  LocationWrite changeset item from `JournalTarget::Map` (summary + path). Both on the single
  review round; 124 tests + clippy + fmt green afterward.
