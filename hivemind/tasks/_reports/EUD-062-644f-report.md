---
task_id: EUD-062-644f
completed_at: 2026-06-05T17:55:00
duration_minutes: 25
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 7
  spec_compliance: 9
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 139000
  output: 35000
cost_usd: 4.70
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Codex-thread isolation from the operator's personal codex environment (live-E2E finding: agent threads inherited the personal `~/.codex` MCP servers, plugins, and skills):

- **`CodexIsolation`** (frozen dataclass, injectable via `CodexSDKRunner(isolation=...)`) drives launch-level `CodexConfig.config_overrides`: (a) WHOLE-TABLE `mcp_servers={...}` override admitting ONLY `eud-tools` (replaces the personal table — playwright/pencil/node_repl excluded), (b) `features.plugins=false`, (c) `extra_overrides` passthrough for the live E2E / future mechanisms.
- **`_toml_inline`/`_toml_key`** — minimal inline-TOML serializer (Windows backslash paths escaped; dashed key `eud-tools` quoted); round-trips through `tomllib` in tests.
- **Two-layer design kept**: the per-thread `thread_start(config={mcp_servers: ...})` layer still carries the live `EUD_REQUEST_ID`/`EUD_DATA_DIR` env (it changes per chat; the launch override is fixed at `Codex` construction). Both layers key the same `eud-tools` table name.
- **Probed live (no tokens)**: `--ignore-user-config` is EXEC-ONLY — `codex app-server --ignore-user-config` errors "unexpected argument"; the SDK spawns `app-server`, so the flag is unusable. `launch_args_override` stays None (guarded by test).
- **Skills NOT isolated (documented limitation)**: no blanket skills-disable exists for app-server; per-skill `[[skills.config]] enabled=false` needs path enumeration. `CODEX_HOME` relocation REJECTED (would diverge the BYO account's `auth.json` token rotation).

Verify-first gate: Step A red committed separately (929512b — AttributeError/ImportError on the isolation surface).

## Changes

`server/eud_agent/agent_runner.py` (+168/-5: docstring section, CodexIsolation, TOML serializer, `_codex_config`/`_isolation_overrides`, constants), `server/tests/test_agent_flow.py` (+125: section 6b, 4 tests + helpers).

## Verification

- Step A red confirmed; merged-tree worktree: pytest **498 passed / 5 skipped**, ruff clean on changed files (single pre-existing unrelated E501 in test_panel_static.py:336 left untouched per surgical-changes).
- SDK source verified by both worker and reviewer: `config_overrides` → `["--config", kv]` BEFORE the `app-server` subcommand (client.py).

## Review

Verdict: approve (no blocking findings). Rubric: correctness 7, spec_compliance 9, safety 9, clarity 9.

Advisories (recorded, all deferred to live E2E by design):
- codex `-c` whole-table REPLACE semantics live in the codex Rust binary — unverifiable from the SDK source; tests prove the override string is well-formed single-key TOML, not that codex applies it as a replace. (CLI-probed live during task grounding; E2E re-confirms.)
- Two-layer reconciliation (launch table without env vs thread table with env, same key) is codex-binary behavior; if the launch entry shadowed env, the shim would lose `EUD_DATA_DIR` discovery. Mitigated by same-key design; MUST be observed in EUD-061 live E2E.
- Skills remain loadable from the personal env — documented limitation until a mechanism exists; `extra_overrides` is the injection point.

## Harness Sync

harness sync: no-op — agent_runner.py already in features/05 `## Implementation`; no manifest changes. Pass.

## Notes

- EUD-061 live E2E must verify: agent thread sees ONLY eud-tools (no personal MCP/plugins), and the shim still receives its env (two-layer reconciliation).
- Worker rebased its worktree onto main (6ca79fd) before starting — base drift noted, merge clean.
