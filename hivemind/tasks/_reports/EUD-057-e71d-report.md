---
task_id: EUD-057-e71d
completed_at: 2026-06-05T15:50:00
duration_minutes: 30
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 8
  spec_compliance: 9
  safety: 7
  clarity: 9
tokens:
  estimated: true
  input: 344600
  output: 86150
cost_usd: 11.63
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Build pipeline per features/05 "Build error retrieval and self-fix", closing the EUD-045 agent-core story:

- **`edd_runner.py`** (new) — `build_run` pipeline: bridge BUILD (already hardened by EUD-052) → poll `status.txt` compiling via `engine.parse_status` (300s injectable timeout) → error ladder: BUILDERR macro errors short-circuit; success = output map exists AND fresh (mtime snapshotted BEFORE the build — absent-now-present or mtime-advanced; mirrors the editor's `LastOupputModifiyTimer` freshness tracking, eliminating the stale-artifact false positive); else direct `euddraft.exe <eds>` re-run (resolved absolute path from `getset program/euddraft` with fail-fast `ConfigError`, explicit `stdin=DEVNULL`, `cwd`=eds dir, `timeout=` injectable 300s with `TimeoutExpired`→`TimeoutError`, `encoding="utf-8", errors="replace"` — no cp949 decode crash). Output parsed with the editor's BuildErrorHandling regexes, replicated exactly (orchestrator-verified against `BuildErrorHandling.vb:21-57`): module form `\[Error.*\] Module "(.*)" Line (\d+) : (.+)` (multi-error loop), traceback form only on zero module matches (LAST description match per the editor's overwrite loop; FIRST file/line frame; basename-without-extension). Structured `BuildError{source: macro|euddraft, file, line, message, raw}`; `last_result` on the runner.
- **`tools.py`** — injectable `runner_factory` (mirrors journal_factory; without one, build_run falls back to plain `bridge.build()` — EUD-052 behavior byte-compatible). Each REAL build attempt consumes one of 3 build-fix attempts; the 4th → ToolError ("self-fix budget spent") + `RequestState.build_fix_exhausted` (in `budget_snapshot`); `ConfigError` (unset euddraft / empty eds) re-raised as ToolError WITHOUT consuming the budget (codex can't fix config by editing eps). `build_errors` returns the structured ladder result.

Verify-first gate: Step A failing suite committed first (ModuleNotFoundError red).

## Changes

- `server/eud_agent/edd_runner.py` (new, ~500), `server/eud_agent/tools.py` (+131), `server/tests/test_edd_runner.py` (new, 23 tests)

## Verification

- Step A red; worker worktree post-review: ruff clean, 492 passed / 5 skipped.
- Merged main tree: ruff clean; **493 passed / 4 skipped**.
- Editor-source grounding orchestrator-verified: both regexes byte-identical to `BuildErrorHandling.vb:23/42/49`; first-frame + basename reduction per `:54-57`.

## Review

Review round 1 (the permitted round) — initial rubric: correctness 8, spec 9, **safety 7 (blocking)**, clarity 9. All findings fixed and orchestrator-re-verified:
- (blocking) euddraft subprocess had NO timeout — a hung euddraft.exe blocked the runner thread forever → injectable `subprocess_timeout` + TimeoutExpired translation; tested.
- (blocking) `text=True` decode used cp949 on Korean Windows — euddraft's UTF-8 output could raise an uncaught UnicodeDecodeError → `encoding="utf-8", errors="replace"`; tested with real UTF-8 Korean output.
- Config errors burned the self-fix budget (3 misconfigurations exhausted it) → `ConfigError` exempt; tested both directions.
- Stale-artifact success false positive → pre-build mtime freshness check; tested.
- Untested tool-layer failure branch → covered (TimeoutError consumes attempt + action + mutation).
- Traceback description aligned to the editor's last-match semantics.

Advisory (documented, live-E2E item): the bridge runs Build SYNCHRONOUSLY inside the Tick handler behind the `IsCompilng` early-return, so `status.txt` may never show `compiling=True` from the server's vantage and the poll may return immediately — the ladder still resolves correctly (BUILDERR + fresh-map check don't depend on observing the flag); confirm timing during the live editor session.

## Incident

### What broke
- Unbounded euddraft subprocess wait; cp949 strict-decode crash path; config errors exhausting the 3-attempt budget; stale-output-map success false positive; first-vs-last traceback description mismatch.

### Why
- The first pass honored the codex-invocation rules for stdin/path/cwd but missed `timeout=`/`encoding=` (the two parameters rules.md doesn't spell out for non-codex subprocesses), and took `path_exists` as the success signal without the editor's freshness dimension.

### What fixed it
- Review round 1 (commit b528c15): injectable subprocess timeout, total UTF-8 decode, `ConfigError` budget exemption, pre-build mtime freshness, last-match alignment, +8 tests.

## Harness Sync

harness sync: no-op (all touched files already documented) — `edd_runner.py`/`tools.py` listed in features/05 `## Implementation`; test file is a test; no manifest changes. Contract-drift guard: additive; pass.

## Notes

- **EUD-045 (agent core) story auto-completed** with this merge: spike → tool layer → journal/rollback → engine/WS v2 → build pipeline.
- Engine wiring of `build_fix_exhausted` into the changeset note is deliberately deferred (engine.py out of scope here) — the flag is exposed on `budget_snapshot` for the panel and for the later engine task.
- Production `create_app` does not yet inject `runner_factory` (feature dormant until the engine/build wiring task turns it on) — consistent with the deferred wiring.
