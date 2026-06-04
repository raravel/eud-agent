---
task_id: EUD-010-e3af
completed_at: 2026-06-04T18:46:57
duration_minutes: 30
coding_retries: 0
verify_retries: 0
review_rounds: 0
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
  input: 3260
  output: 4400
cost_usd: 0.38
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
PowerShell 7 deployment scripts: install_dropin.ps1 (validate-before-copy against `EUD Editor 3.exe` + `Data\Lua\TriggerEditor`; copies lua + 3 DLLs; writes BOM-free agent.cfg `{python_exe, repo_root, port}` with repo_root from $PSScriptRoot; idempotent), uninstall_dropin.ps1 (lua + Data\agent; DLLs kept unless -RemoveDlls), setup_env.ps1 (uv sync + sanity import + bge-m3 cache warn; -Cpu prints guidance), dev_run.ps1 (temp EUD_DATA_DIR; honest exit-code propagation). Two-phase verify-first gate followed (Step A RED confirmed by orchestrator before Step B).

## Changes
- `scripts/install_dropin.ps1`, `scripts/uninstall_dropin.ps1`, `scripts/setup_env.ps1`, `scripts/dev_run.ps1` (new); `scripts/.gitkeep` removed
- `server/tests/test_deploy_scripts.py` — 7 functional checks driving real pwsh runs against TEMP fake-editor layouts

## Verification
- Verify-first gate: Step A confirmed RED by orchestrator (0/7, exit 1) before Step B authorization; GREEN after — 7/7 run by orchestrator (real subprocess runs: install/idempotent/wrong-path-refusal/uninstall/setup_env/dev_run-timeout-guarded).
- Worker also ran the full suite in its worktree: pytest 69 passed, ruff clean.
- Scope-drift gate: 6 paths via merge-base diff, all declared.
- Real editor folder untouched (tests use fake layouts only; reviewer confirmed install validates BEFORE first Copy-Item — no partial-install path).

## Review
Verdict PASS (9/9/8/10), no blocking findings. Reviewer verified: BOM-free agent.cfg with correct JSON backslash escaping (round-trip tested); cwd-independent repo_root; genuine idempotency; wrong-path test proves zero leakage; dev_run does not mask the current "app not implemented" exit 2; CP949→UTF-8 console workaround is process-scoped and sound; Step A→B test modifications were lint/encoding only — NO assertion weakened (gate not gamed). Advisories: (1) `-RemoveDlls` matches by basename and could delete the editor's own pre-existing WebView2 DLLs (identical bytes today; off-by-default + documented) — add a warning line on next touch; (2) `-Cpu` is guidance-only by design (documented; auto-editing pyproject would be fragile); (3) agent.cfg may reference a not-yet-created venv python (warned, install-before-setup tolerated by design).

## Harness Sync
- features/02_python-server.md += scripts/setup_env.ps1 (a9897f7), += scripts/dev_run.ps1 (07fecd5)
- features/01_lua-bridge.md += scripts/uninstall_dropin.ps1 (51c7012)
- install_dropin.ps1 already listed in features/01 ## Implementation; test excluded; no manifest changes. Contract-drift clean.

## Notes
- Parent story EUD-002-684a auto-completed (scaffolding/import phase finished).
- Carry-forward: when the React panel lands, dev_run.ps1 usage docs gain the `npm --prefix panel run build` precondition (verify.md panel stage already records it).
- Raw harness-reported subagent tokens ≈ 208,750 (54,361 + 96,764 + 57,625).
