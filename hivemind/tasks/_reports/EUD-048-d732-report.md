---
task_id: EUD-048-d732
completed_at: 2026-06-05T11:45:21
duration_minutes: 10
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: false
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 10
  spec_compliance: 10
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 101518
  output: 25379
cost_usd: 3.43
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

SCA (SCArchive / scarchive.kr publish service, incl. the SCAScript file type) is fully defunct (user decision 2026-06-05). Dropped "SCA" from the server's settable file-type families, added a static regression test guarding against reintroduction, and scrubbed the CUI/SCA/RawText wording from harness docs. rules.md gained the SCA-defunct NEVER rule (incl. forcing `pj.TEData.SCArchive.IsUsed = false` before BUILD).

## Changes

- `server/eud_agent/bridge_io.py` — `_SETTABLE_FAMILIES = ("CUI", "RAWTEXT")`; comment block + `set()`/`list_files()` docstrings scrubbed of SCA. CUIPy/CUIEps/CUITrg still match via the "CUI" substring.
- `server/tests/test_bridge_list_static.py` — new `test_settable_families_exclude_sca`: static source check that no `_SETTABLE_FAMILIES` member contains "SCA" (catches a hypothetical "SCAEps" too).
- `server/tests/test_bridge_io.py` — `test_list_files_parses_tab_lines_with_settable` now asserts the `sca/mod.sca\tSCA` line is `settable is False` (kept as a negative regression case). Scope-add pre-approved by the orchestrator (no in-flight peers).
- `hivemind/docs/rules.md` — SET/NEWEPS setter line → CUI/RawText; new Editor-integrity NEVER rule: SCA defunct, never reintroduce as settable/creatable, always force `SCArchive.IsUsed = false` before BUILD.
- `hivemind/docs/architecture.md` — SET row → "CUI/RawText only (GUI has no setter; SCA defunct)".
- `hivemind/docs/features/02_python-server.md` — `list_files` settable note → CUI/RawText.

## Verification

Run by the orchestrator in the worker worktree (base 23bc6f4):
- `python -m ruff check server` → All checks passed!
- `python -m pytest server/tests -q` → 242 passed, 4 skipped

Re-run on the merged main tree (worker base was 4 commits behind main; auto-merge clean in `rules.md` and `test_bridge_list_static.py`):
- `python -m ruff check server` → All checks passed!
- `python -m pytest server/tests -q` → 247 passed, 3 skipped

Scope-drift gate: all 6 touched files within declared scope (after orchestrator scope-add of `server/tests/test_bridge_io.py`). Contract-drift guard: no removed identifiers/signatures; the rules.md addition is the task's specified contract (features/04 Scope decisions). Pass.

## Review

Verdict: approve, no blocking findings. Rubric: correctness 10, spec_compliance 10, safety 10, clarity 9.

Advisory findings (optional hardening, not applied):
- The static test's `_SETTABLE_FAMILIES` extraction regex (`\(([^)]*)\)`) is single-line-only; a future multiline tuple with a `)` in an inline comment would truncate extraction — but the test's own asserts would then fail loudly rather than pass silently.
- Edited docs use the "CUI/RawText" family shorthand while features/04 enumerates CUIEps/CUIPy/RawText — consistent with the pre-existing shorthand style and the substring-family model; not drift.

## Harness Sync

harness sync: no-op (all touched files already documented) — `bridge_io.py` is already listed in features/04 `## Implementation` (including the `_SETTABLE_FAMILIES` SCA drop itself); no manifest changes. The harness-doc edits in this diff are the task's own scope (harness:rules/architecture/features), not binding sync.

## Notes

- The coding worker's worktree was created from 23bc6f4 (4 commits behind main); the squash-merge 3-way resolved `rules.md` (EUD-039 DispatcherTimer rule preserved alongside the new SCA rule) and `test_bridge_list_static.py` (EUD-040 tests preserved) cleanly. Merged-tree verification confirmed both.
- The worker created a gitignored junction `server/.venv` inside the worktree to run pytest against the main venv (3 `test_deploy_scripts.py` tests assert the cwd-derived venv path exists). Not part of the commit.
- Deferred this round (scope conflict with this task): EUD-053-f3ac (overlap `*`), EUD-042-45cb.
