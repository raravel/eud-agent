---
task_id: EUD-018-bafb
completed_at: 2026-06-04T20:21:54
duration_minutes: 80
coding_retries: 0
verify_retries: 0
review_rounds: 1
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
  input: 4900
  output: 7300
cost_usd: 0.62
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Server integration core. orchestrator.py: async per-instruct state machine rag(optional)→codex→lsp(lazy import, "skipped" when absent)→diff→done with exact WS protocol events; single-flight instruct via asyncio.Lock held across the whole run (second → error "busy"); sync bridge/rag via to_thread; BridgeBusy → waiting_build progress (run_coroutine_threadsafe hop) + "editor busy" error; RagUnavailable degrades to no-context; codex-None guard (post-review). app.py: create_app with injectable lifecycle; GET / serves panel/dist (503 + npm-build hint when unbuilt); /healthz; WS /ws token+Origin validated BEFORE accept (close 4403); two-client broadcast; ready-writer thread (atomic temp+replace, BOM-free, written only after a real TCP self-connect, deleted on shutdown AND staleness); unkillable HeartbeatWatcher (post-review) self-terminating uvicorn via should_exit; RAG warmup kicked at startup. __main__.py: no-flag → _serve with the PRE-BOUND-socket pattern (resolve port incl. 0-fallback BEFORE create_app so Origin/ready advertise the real port; Server.run(sockets=[sock]) — single bind, no race).

## Changes
- `server/eud_agent/orchestrator.py`, `app.py` (new), `__main__.py` (no-flag wiring; --selfcheck untouched)
- `server/tests/test_orchestrator.py` (16), `test_app.py` (13 incl. real-uvicorn integration, 0.5s)
- `server/tests/test_config.py` + `test_deploy_scripts.py` — 2 obsolete EUD-010-stub tests retargeted to the serve behavior (scope-add approved; reviewer confirmed strictly-stronger assertions, gate not gamed)

## Verification
- Two-phase gate: Step A RED (2 collection ImportErrors) confirmed by orchestrator before Step B; post-fix GREEN — 194 passed + 2 skipped + ruff clean re-run independently; real-server integration test re-run independently (server.ready only after socket accepts; ready.port == resolved port; deleted on shutdown).
- Scope-drift gate: 7 paths, all declared (2 via approved scope-add).

## Review
Verdict PASS (9/9/8/10), no blocking. One advisory-driven round: (1) HeartbeatWatcher loop body unguarded — an unforeseen exception would silently kill the only thread guaranteeing the server never outlives the editor; now try/except-wrapped with a survival test (on_stale raising once → re-fires next tick); (2) codex=None instruct → clean error event instead of AttributeError into the WS loop. Reviewer also verified: pre-bound socket honors should_exit (uvicorn 0.49.0 source); _parse_status maps the bridge STATUS text → {compiling, project} in the orchestrator (bridge_io stays raw); Step A→B test diffs strictly stronger.

## Incident
### What broke
- Review found the unguarded watcher loop (rules.md "never outlive the editor" single point of failure) and the codex-None AttributeError path.
### Why
- The watcher's per-component robustness masked the loop-level gap; codex-None was assumed unreachable behind selfcheck.
### What fixed it
- One round (c96c4d7): loop-body try/except + stderr log + survival test; top-of-instruct codex-None gate + clean-error test.

## Harness Sync
- features/02 += one-line instruct-target contract note (reviewer recommendation): instruct `target` must be an existing settable file; new files only via apply mode=neweps.
- All three modules already in features/02 ## Implementation; no manifest. Contract-drift clean (retargeted test identifiers absent from spec corpus; the EUD-010-stub behavior they asserted was explicitly superseded by this task's spec).

## Notes
- CARRY-FORWARD → EUD-034 (appended later if needed): browsers observe the pre-accept 4403 as a failed handshake (1006) — the panel must treat ANY close as retry, never branch on 4403 (EUD-033's client already does blanket-retry).
- KNOWN DRIFT (owned by EUD-035 per plan): config.run_selfcheck still checks the flat vanilla PANEL_FILES, not panel/dist — the documented transition state.
- dev_run.ps1 header comment still says "app not implemented" — stale cosmetic text, EUD-010-owned; behavior correct (script now launches the real server; its retargeted test proves server.ready appears).
- @app.on_event deprecated under pinned FastAPI — works today; migrate to lifespan on a future bump.
- Raw harness-reported subagent tokens ≈ 527,119 (90,707 + 158,179 + 106,278 + 171,955).
