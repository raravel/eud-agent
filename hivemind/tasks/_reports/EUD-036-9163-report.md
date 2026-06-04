---
task_id: EUD-036-9163
completed_at: 2026-06-04T22:21:54
duration_minutes: 20
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 10
  spec_compliance: 10
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 2400
  output: 3900
cost_usd: 0.33
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Boot-handshake bug fix (found by the orchestrator while chasing the EUD-030 reviewer's dead-seam finding, BEFORE the editor e2e could hit it): the bridge spawned the server with `psi.Arguments = "-m eud_agent"` — no data-dir signal — so cfg.data_dir resolved empty and server.ready/heartbeat/inbox landed relative to the server cwd; the bridge (polling `<editor>\Data\agent\server.ready`) would never validate readiness and the panel would never navigate. Fix: `psi.Arguments = '-m eud_agent --data-dir "' .. string.sub(agentDir, 1, -2) .. '"'` — trailing backslash stripped so the closing quote is not escaped on the CreateProcess command line.

## Changes
- `bridge/ZZZ_10_agent_bridge.lua` — one hunk in spawnServer (+3/-1)
- `server/tests/test_bridge_lifecycle_static.py` — new spawn-args check (bound to the psi.Arguments assignment; requires --data-dir + quoted `..` concatenation + the trailing-strip marker; mutation-verified OLD-fails/NEW-passes)

## Verification
- Two-phase gate: Step A RED (14/15, exactly the new check failing) confirmed by orchestrator; GREEN after (15/15 + the other 3 bridge static suites, re-run independently).
- LIVE proof by orchestrator on the main venv: `python -m eud_agent --data-dir <tmp> --port 0` → server.ready written INTO <tmp> (CLI flag routes end-to-end).
- Scope-drift gate: 2 paths, both declared.

## Review
Verdict PASS (10/10/10/9), no blocking. Reviewer EMPIRICALLY reproduced both directions with a realistic spaces-containing editor path (`C:\Program Files\EUD Editor 3\Data\agent\`): the un-stripped variant's `\"` swallows the path + next token (the bug the strip prevents is real and load-bearing); the fixed form parses through CreateProcess/MSVCRT/argparse intact. string.sub provably safe; the only quoted command-line value in the file; every Python-side consumer joins via Path (trailing-slash-free value breaks nothing); 6-variant mutation test of the regex confirmed binding; committed blobs pure LF/ASCII.

## Harness Sync
- no-op: bridge lua in features/01 ## Implementation; test excluded; no manifest. Contract-drift clean.

## Notes
- ORIGIN: cross-task integration gap (EUD-013 spawn × EUD-009/018 config/app data_dir contract) invisible to every per-task suite — caught by orchestrator tracing during merge review, validating the pipeline's verify-yourself discipline. The editor e2e (EUD-024) boot-handshake step 1 should now pass on the first attempt.
- Raw harness-reported subagent tokens ≈ 169,034 (47,146 + 61,316 + 60,572).
