---
task_id: EUD-030-75a4
completed_at: 2026-06-04T22:12:50
duration_minutes: 35
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 10
  clarity: 10
tokens:
  estimated: true
  input: 3200
  output: 5100
cost_usd: 0.43
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Config robustness chore (EUD-009 advisories 1-3): `Config.warnings` (non-fatal diagnostics — auto-discovered broken cfg, non-numeric port falling back to 8765) + `Config.cfg_error` (explicitly-pointed-at cfg exists but unparseable → "agent.cfg present but unparseable at <path>", selfcheck non-zero); `_locate_agent_cfg` returns (path, explicit) distinguishing direct-path/CLI/EUD_DATA_DIR (explicit) from the new `default_data_dir` seam (auto); `_load_agent_cfg` returns (dict, parse_failed) catching BOM'd/malformed/non-dict under plain utf-8 (utf-8-sig stays forbidden per rules); HF cache resolution standardized to HF_HUB_CACHE > HF_HOME/hub > default with the invented HF_HOME_HUB removed; `__main__._selfcheck` prints `! warning:` lines on the OK path.

## Changes
- `server/eud_agent/config.py` (+138/-24), `server/eud_agent/__main__.py` (+4), `server/tests/test_config.py` (+191; 13 new tests, zero removed lines)

## Verification
- Two-phase gate: Step A RED (13 new failures / 18 existing green) confirmed by orchestrator before Step B; GREEN after — 209 passed + 3 skipped + ruff + selfcheck exit 0, re-run independently. Worker spot-checked the explicit-broken-cfg path live (EUD_DATA_DIR → exit 1 with the distinct message); reviewer reproduced it independently.
- Scope-drift gate: 3 paths, all declared.

## Review
Verdict PASS (9/9/10/10), no blocking. Reviewer traced every resolve() entry path — no broken cfg is silently swallowed and no path is misclassified; run_selfcheck's (code, messages) contract unchanged for all consumers; the test diff is purely additive. Advisories: (1) the auto-discovery warning seam (`default_data_dir`) is DEAD IN PRODUCTION — nothing wires it (note: superseded by the EUD-036 finding below; the bridge will pass --data-dir explicitly, which is the explicit path, so the auto seam remains a test-covered fallback); (2) CLI --port is argparse type=int so CLI garbage hard-fails before the guard (env/cfg-only net — intended); (3) HF empty-env handling consistent with pick().

## Harness Sync
- no-op (skip condition): config.py/__main__.py in features/02 ## Implementation; tests excluded; no manifest. Contract-drift clean (the chore makes verify.md's "agent.cfg schema" claim true).

## Notes
- INTEGRATION BUG DISCOVERED during this merge (orchestrator, chasing the reviewer's dead-seam finding): the bridge spawns the server with NO data-dir signal (`psi.Arguments = "-m eud_agent"`), so cfg.data_dir resolves empty and server.ready/heartbeat/inbox land relative to the server cwd — the editor boot handshake would NEVER complete. Filed as EUD-036-9163 (bug, high): bridge passes `--data-dir "<Data\agent>"`; __main__ already accepts it. This is the kind of gap only the editor e2e would have surfaced — caught pre-e2e by review-driven tracing.
- Raw harness-reported subagent tokens ≈ 235,983 (71,776 + 91,961 + 72,246).
