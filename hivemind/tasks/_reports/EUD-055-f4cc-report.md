---
task_id: EUD-055-f4cc
completed_at: 2026-06-05T14:31:05
duration_minutes: 35
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 5
  spec_compliance: 7
  safety: 8
  clarity: 9
tokens:
  estimated: true
  input: 345822
  output: 86456
cost_usd: 11.67
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Change journal + rollback engine (features/05 "Change journal and rollback"):

- **`journal.py`** — before-snapshots per write kind through the SAME bridge (dat family via the matching GET, parsed with the unit-tested `parse_get_value` first-" = " splitter; file_write/file_delete old content via GET with a HARD guarantee — a failed snapshot GET fails the call, the write is not performed; file_create/mkdir created markers; rename/move old paths; set_main via GETMAIN; settings via GETSET; plugin_add created index; **dat_reset** snapshots the pre-reset value per kind and re-reads the stock value after). Entries `{id, seq, tool, target, before, after, ts}` persisted atomically (temp+os.replace, UTF-8 no BOM) to `<data-dir>/journal/<request-id>.json` after every write; `Journal.load` reproduces an identical changeset. Changeset per WS v2 (file kinds created|modified|deleted + unified diff for modified; dat-kind incl. dat-resets grouped per objId). Rollback replays inverse ops in REVERSE seq order (dat/xdat/tbl/req/btn old-value writes; RESETDAT when was_default; DELFILE for created; NEWFILE+content for deleted; RENAME/MOVEFILE/SETMAIN/SETSET back; plugin inverses); accept/finalize archives to `<request-id>.accepted.json` with an undecided-defaults-accepted note; mixed per-item decisions supported.
- **Honest limitations (documented in the module docstring, refusal not corruption)**: plugin_edit/remove snapshots are `partial: true` (PLUGLIST exposes only first lines; no full-Texts GET) — rollback refuses those items per-item; set_main with NO prior main refuses rollback (no clear-main primitive); was_default has a real signal only for tbl (empty GETTBL = default → RESETDAT inverse), dat/xdat/req/btn record false; file_create/mkdir rollback does not delete auto-created parent folders; deleted-file type is extension-inferred on recreate.
- **`tools.py`** (additive) — optional `journal_factory`; journaled write order: unknown-tool → budget → gate → `_validate_args` (handler run against a no-op `_ProbeBridge`; zero bridge traffic on invalid args) → snapshot (BridgeError→ToolError translated) → bridge write → record after. Rejections leave no entry; build_run is gate/budget-counted but NOT journaled (no vacuous changeset items); journal-less construction byte-identical.

Verify-first gate: Step A committed the failing suite first (ModuleNotFoundError red, test-only commit).

## Changes

- `server/eud_agent/journal.py` (new, ~780 lines after review fixes)
- `server/eud_agent/tools.py` (+~130: journal wiring, _ProbeBridge, snapshot error translation)
- `server/tests/test_journal.py` (new, 46 tests)

## Verification

- Step A: test-only commit 54eb771, ModuleNotFoundError red.
- Worker worktree (local uv venv): ruff clean; pytest 459 passed / 4 skipped (447 pre-review + 12 review-round tests); journal suite 46/46.
- Merged main tree: ruff clean; **460 passed / 3 skipped**.

## Review

Review round 1 (the one permitted round) — initial rubric: correctness 5 (BLOCKING), spec 7, safety 8, clarity 9. All findings fixed and orchestrator-re-verified:

- **BLOCKING — `dat_reset` journaled but snapshot/changeset/inverse unimplemented** (silent loss of the recoverable pre-reset value; garbage changeset group; rollback refusal). Fixed: `_read_reset_target` snapshots via the matching GET per kind; `compute_after` re-reads the stock value; changeset groups kind=dat resets per objId; inverse writes the old value back via dat_set/xdat_set/settbl. 4 new tests.
- build_run journaling was vacuous → excluded from journaling (still gate/budget-counted); 2 tests.
- set_main empty-prior-main inverse was an incidental BridgeError → explicit partial-marker refusal naming the missing clear-main primitive; tested.
- `_safe_get` masked snapshot-GET failures as `content=""` (a later rollback would EMPTY the file — silent-corruption path) → removed; snapshot GET failure now fails the call before any write; 2 tests.
- Snapshot BridgeError leaked untranslated → wrapped to ToolError.
- Coverage: xdat_set/req_set/btn_set through-the-layer round-trips added — which surfaced and fixed a latent `inverse_dat_op` crash (`args["objId"]` read unconditionally; btn_set keys on `setId`).

## Incident

### What broke
- `dat_reset` rollback data was silently lost (blocking); `_safe_get` created a rollback-corrupts-file path; `inverse_dat_op` had a latent btn_set KeyError.

### Why
- The first implementation treated dat_reset/build_run as default-branch fall-throughs instead of explicit cases, and traded snapshot-GET failures for "best-effort" empty content. The review's per-kind inverse trace caught all three; the btn_set crash was exposed by the review-mandated coverage, not by inspection.

### What fixed it
- Review round 1 (commit c9b39d3): explicit dat_reset snapshot/after/changeset/inverse; build_run journal exclusion; hard snapshot-before-mutate guarantee; per-branch objId/setId keying; 12 new tests.

## Harness Sync

harness sync: no-op (all touched files already documented) — journal.py/tools.py listed in features/05 `## Implementation`; test file is a test; no manifest changes. Contract-drift guard: additive; nothing spec-promised removed. Pass.

## Notes

- features/05 lists build_run under "Write (journaled)"; the implementation deliberately exempts it from JOURNALING (kept under the gate/budget) because a build has no inverse and a permanent always-failing reject item misleads the panel — spec wording could be refined to "Write (gated)" vs "Write (journaled)" when the docs are next refreshed.
- The journal currently snapshots through the live bridge synchronously per write (2 extra IPC round-trips per mutation: before-GET + after-read for resets). Acceptable at the 1s-Tick cadence; revisit only if tool throughput matters later.
