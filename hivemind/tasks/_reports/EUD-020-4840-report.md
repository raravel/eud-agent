---
task_id: EUD-020-4840
completed_at: 2026-06-04T22:19:11
duration_minutes: 50
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 8
  spec_compliance: 9
  safety: 8
  clarity: 8
tokens:
  estimated: true
  input: 4100
  output: 6400
cost_usd: 0.54
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
`server/eud_agent/runner_cli.py` (290→clean post-round): headless retention of the verified runner — `--once/--mock/--no-context/--data-dir` (legacy `--agent-dir` alias), jobs queue (`jobs/*.json` {instruction, target, context} → `inbox/agent_<id>.cmd` "SET <target>\n<code>" UTF-8 no BOM via encode+write_bytes; `.json`→`.done` os.replace), per-job failure isolation, RAG degrade on RagUnavailable, all heavy lifting through the shared modules (Config.resolve / rag.search / codex_client.build_prompt+extract_code+CodexClient). Post-review: the identity-gated mock mechanic replaced with a `_make_codex_client(cfg, *, mock)` factory — mock returns an in-process `_MockCodexClient` stub (prompt-observable, codex-less by construction), eliminating the latent silent-real-codex-under---mock trap.

## Changes
- `server/eud_agent/runner_cli.py` (new), `server/tests/test_runner_cli.py` (8 tests; T4 strengthened from source-grep to CALL-LEVEL assertions on build_prompt/extract_code)

## Verification
- Two-phase gate: Step A RED (collection ImportError) confirmed by orchestrator; GREEN after — 203 passed + 3 skipped + ruff clean, re-run independently post-round. runner_legacy byte-identity intact throughout.
- Scope-drift gate: 2 paths, both declared.

## Review
Initial PASS on thresholds (8/9/8/8) with one advisory-driven round: F1 — the `type(client) is _REAL_CODEX_CLIENT` identity gate would let a subclassed/reloaded client silently spawn REAL codex under --mock (latent; failure mode = silent user-account-consuming subprocess); F2 — redundant async ceremony on the mock path; F6 — source-text-grep structural tests pressuring docstring wording. Round (10339c4): factory seam (`_make_codex_client`), branch-free `_generate_code`, call-level T4 split into three tests, natural docstrings restored. Reviewer had verified pre-round that no subprocess could spawn under mock TODAY and no mock leak into real runs existed — the round removed the latent class, not an active bug.

## Incident
### What broke
- Review flagged the identity-gated mock mechanic as a refactor trap (silent real-codex spawn) and the grep-based structural tests as wording-pressure smells.
### Why
- The Step-A test pinned "mock must construct CodexClient," steering the implementation into identity branching to satisfy observability without a subprocess.
### What fixed it
- One round: module-level factory seam + call-level test assertions (tests now patch the factory; the runner's own stub is exercised unpatched in one test).

## Harness Sync
- no-op (skip condition): runner_cli.py already in features/02 ## Implementation; tests excluded; no manifest. Contract-drift clean.

## Notes
- The factory's real branch (one-line CodexClient construction) has no dedicated test — flagged by the worker as optional; acceptable (config precedence + codex_client suites cover the constituents).
- Raw harness-reported subagent tokens ≈ 404,103 (81,360 + 114,283 + 72,525 + 135,935).
