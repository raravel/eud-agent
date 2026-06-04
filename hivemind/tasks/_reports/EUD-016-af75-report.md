---
task_id: EUD-016-af75
completed_at: 2026-06-04T19:31:17
duration_minutes: 55
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 7
  spec_compliance: 10
  safety: 6
  clarity: 9
tokens:
  estimated: true
  input: 4110
  output: 6030
cost_usd: 0.51
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
`server/eud_agent/codex_client.py`: CodexClient with fail-fast path validation (CodexNotFound), async generate() via create_subprocess_exec(resolved, "exec", "--skip-git-repo-check", PIPES, cwd=repo_root) — full prompt via stdin then close (guarded against BrokenPipeError/ConnectionResetError with fallthrough to communicate() after the review round), kill+await wait() on timeout (CodexTimeout), fence extraction with line-anchored closing fences + CRLF→LF normalization (CodexNoCode carries ≤500 chars raw + stderr tail), build_prompt composer, SYSTEM_PROMPT copied byte-identical from runner_legacy (provenance comment; no import). Empirical finding: direct asyncio exec of the codex .CMD shim works on Windows ProactorEventLoop — no cmd.exe fallback needed.

## Changes
- `server/eud_agent/codex_client.py` — new (+236, then fix +114/-11)
- `server/tests/test_codex_client.py` — 20 tests (19 mock + 1 live-gated via EUD_CODEX_LIVE=1)

## Verification
- Two-phase gate: Step A RED (collection ImportError) confirmed by orchestrator before Step B; post-fix GREEN — 129 passed + 1 skipped + ruff clean, re-run independently by orchestrator.
- LIVE codex round-trip run by the orchestrator TWICE (pre-fix and post-fix with the hardened regex): real codex returned fenced code, extraction non-empty — completion criterion proven on this machine.
- Scope-drift gate: 2 paths, both declared.

## Review
Initial verdict BLOCKED (7/10/6/9): safety 6 < 8 and finding B1 — reviewer REPRODUCED a BrokenPipeError [WinError 109] leaking from generate() when codex exits before reading a 300KB prompt (the documented RAG-context case). Also: A1 un-reaped process after timeout kill; A2 fence regex truncating blocks containing inline ``` in string literals (silent truncated-code-applied risk); A3 interior \r surviving into SET bodies. Reviewer additionally verified the write→drain→close→communicate pattern is deadlock-safe (drain waits only on the asyncio high-water mark) and SYSTEM_PROMPT byte-fidelity (ast-compared).
Fix round (9d2d562): all four addressed with regression tests that genuinely exercise the guarded paths (injected drain_error, kill-then-wait order witness, inline-fence preservation, CRLF normalization). Merged after orchestrator re-verification.

## Incident
### What broke
- Review reproduced an unhandled BrokenPipeError escaping generate() on early codex exit with a large stdin payload; plus un-reaped timeout kills and two silent-corruption extraction edge cases.
### Why
- The stdin write/drain/close block was unguarded (the docstring addressed the EOF-hang, not early-exit); the FakeStdin test double never modeled OS-pipe failure, so the suite was structurally blind to the leak.
### What fixed it
- One review round: try/except (BrokenPipeError, ConnectionResetError) fallthrough to communicate(); async kill-and-reap; line-anchored closing-fence regex; CRLF→LF normalization — each with a test that fails on the pre-fix code.

## Harness Sync
- no-op (skip condition): codex_client.py already in features/02 ## Implementation; test excluded; no manifest. Contract-drift clean.

## Notes
- LESSONS (could NOT be saved — `hv feedback save` non-binding targets are unusable this session: every attempt collides with the unrelated L2 `never-bypass-hvtask-agent-team-pipeline...` at scores 3.38/3.56/7.86; gate bug, not content):
  1. (L2 candidate) Guard asyncio-subprocess stdin write/drain/close with except (BrokenPipeError, ConnectionResetError) → fall through to communicate(); a child exiting before reading a >64KB payload raises mid-drain and leaks past custom wrappers; communicate() surfaces the real exit/stderr. Reproduced on Windows ProactorEventLoop with a .cmd shim.
  2. (L2 candidate) Markdown fence extraction must anchor the CLOSING fence to line start (re.M); non-greedy matching stops at inline backtick runs inside string literals and silently truncates extracted code.
  3. (project note) Direct asyncio exec of npm .cmd shims works on Windows/ProactorEventLoop — no cmd.exe /c wrapper needed (empirically proven via live codex).
- Raw harness-reported subagent tokens ≈ 301,369 (64,384 + 77,203 + 61,385 + 98,397).
