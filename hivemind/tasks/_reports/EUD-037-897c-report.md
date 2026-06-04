---
task_id: EUD-037-897c
completed_at: 2026-06-04T23:59:00
duration_minutes: 25
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 7
tokens:
  estimated: true
  input: 4000
  output: 6500
cost_usd: 0.55
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
First defect found by the LIVE editor E2E (EUD-024 boot-handshake step, real editor running): the bridge spawns the server through the venv launcher `server\.venv\Scripts\python.exe`, which on Windows re-execs the base interpreter as a CHILD process (observed: bridge-spawned PID 29472 = launcher; real server PID 34176 = child owning the 8765 socket). The server wrote `"pid": os.getpid()` (child) into server.ready; `validateReady()` compared it to `agentProc.Id` (launcher), mismatched, took the stale branch, and DELETED server.ready on every write — `agentSrvReady` never flipped, the panel never navigated, while the server itself was healthy (HTTP 200, React dist served). Invisible to every headless suite (the runtime rig spawned the server directly, no launcher hop; the bridge check is static-only).

Fix: app.py ready payload adds `"ppid": os.getppid()` (= the launcher pid as seen from the child); bridge `validateReady()` accepts ownership when ownPid matches EITHER `pid` OR `ppid` (nil-safe disjunction, anchored `'"ppid"%s*:%s*(%d+)'` extraction; the existing `'"pid"'` pattern is provably order-independent — its leading quote cannot occur inside `"ppid"`), and deletes server.ready ONLY when neither matches. mtime>bridgeStart freshness check unchanged.

## Changes
- `server/eud_agent/app.py` — ppid in the ready payload + docstring contract note (+5/-1)
- `bridge/ZZZ_10_agent_bridge.lua` — validateReady pid-or-ppid acceptance, neither-match delete (+13/-3)
- `server/tests/test_app.py` — ready payload pins `ppid == os.getppid()` (in-process writer)
- `server/tests/test_bridge_lifecycle_static.py` — `_validate_ready_body()` extractor + 3 binding static checks (ppid string.match marker; >=2 `== ownPid` joined by `or`; File.Delete positioned after both comparisons)

## Verification
- Two-phase gate: Step A RED (exactly the 4 new artifacts fail, 26 pass) confirmed by orchestrator; GREEN after Step B.
- Full suite on merged main: ruff clean; **240 passed, 3 skipped**; selfcheck OK. (Worktree runs showed 3 test_deploy_scripts failures — verified pre-existing environment artifact: install_dropin.ps1 derives repo root from $PSScriptRoot, and worktrees have no server/.venv, so the agent.cfg python_exe existence assert fails there; same 3 tests pass 7/7 on main. Unrelated to this diff.)
- LIVE proof by orchestrator (worktree code via PYTHONPATH, spawned through the real venv launcher exactly like the bridge): launcher pid 134384 → ready.pid 147436 (child), ready.ppid 134384 == launcher → the new accept condition matches via ppid; ppid presence itself proves the worktree code loaded.
- Scope-drift gate: 4 paths, all declared.

## Review
Verdict PASS (9/9/9/7), no blocking. Reviewer verified: Lua 5.1/KopiLua multi-line `if (A) or (B) then` soundness; character-level order-independence of both patterns in both JSON field orders; nil-safety; no LUANET-* violation, no non-ASCII bytes; false-accept scenarios closed (pre-spawn ready delete + mtime gate + respawn latch reset; a foreign process matching via ppid would need to be a direct child of the exact spawned launcher); launcher-exit edge covered by maybeRespawn (the venv launcher waits on its child for its whole lifetime). Advisories: F1 — the Step A test docstring carried a WRONG rationale (claimed the naive `"pid"` pattern matches the tail of `"ppid":` — it cannot; the orchestrator had injected that wrong premise into the Step A prompt and the worker copied it) — fixed directly by the orchestrator before merge (commit in squash); F2 — launcher-lifetime assumption documented; F3 — architecture.md payload mention stale — handled in harness sync.

## Harness Sync
- Contract-drift guard: clean (additive field; no removed identifiers/signatures/rule contradictions).
- Binding appends: no-op (bridge lua in features/01, app.py in features/02; tests excluded; no manifest).
- Doc honesty updates (orchestrator, bundled in this commit): architecture.md (mermaid edge, boot flowchart, server.ready bullet now carry ppid + rationale); features/01_lua-bridge.md (validateReady pid-or-ppid + ALSO restored the EUD-036 `--data-dir` spawn-args wording that had been left stale); features/02_python-server.md (ready payload tuple).

## Notes
- The bridge deployed to the editor BEFORE this fix is broken for venv spawns — reinstall via scripts/install_dropin.ps1 after merge (done in-session) and restart the editor.
- The leftover groundwork probe `ZZZ_11_webview2_probe.lua` was found still installed in the editor's TriggerEditor folder (it produced a confusing static "panel" window); deleted in-session. install_dropin.ps1 does not manage that file — one-time cleanup, no script change needed.
- Raw harness-reported subagent tokens ≈ 199,674 (coder 65,373 + 77,910; reviewer 56,391).
