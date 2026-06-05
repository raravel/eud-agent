---
task_id: EUD-064-acf5
completed_at: 2026-06-05T18:55:00
duration_minutes: 35
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 8
  spec_compliance: 10
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 240000
  output: 76000
cost_usd: 9.30
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Conversation continuity (user bug report: "the agent forgets what I just said" — every `chat` was spawning a brand-new codex thread via `thread_start`; only the plan loop resumed):

- **Engine start-vs-resume routing** (`engine.py`): the FIRST chat starts the thread (system prompt as `base_instructions`); every later chat RESUMES it (`has_thread` query, defensive getattr so legacy fakes keep always-start). Resumed chats prepend refreshed `[project state]` + `[reference context]` (RAG for the new question) to the turn text via `_resume_turn_text` (reuses the system-prompt section builders).
- **Runner retention guard** (`agent_runner.py`): `_run_turn_blocking` resumes whenever a thread is retained — even a stray `start_turn` cannot discard history; a fresh `thread_start` happens only with no retained thread (first chat / after reset). ABC gains `has_thread`/`reset_thread`; `reset_thread` keeps the lazily built (isolated) app-server process alive.
- **`reset{}` WS message**: `executing`/`plan_review` → error (cancel first; consistent with chat gating); `changeset_review` → finalize prior journal (default-accept + archive note) then drop; `idle` → idempotent drop. Clears the published request id and `_plan_revision`.
- **Live request-id stamping** (`app.py`): the engine publishes its CURRENT request id (`register_request_id`) to `app.state.active_request_id`; `/tools/call` stamps it over the stale shim env id (pinned at thread creation, stale from chat #2 on); `None` → shim id fallback (legacy headless). Endpoint also reads the test-swappable `app.state.tool_layer`. Cleared on reset and `aclose`.

Verify-first gate: Step A red committed separately (20c192d, 7 failed / 502 passed) — orchestrator re-ran the red commit (7 failed) and HEAD (green) directly.

## Changes

`server/eud_agent/engine.py` (+90), `server/eud_agent/agent_runner.py` (+60: continuity + docstring premise rewrite), `server/eud_agent/app.py` (+27), `server/tests/test_agent_flow.py` (+395), `server/tests/test_app.py` (+171). mcp_shim.py/tools.py untouched (stamping is server-side by design).

## Verification

- Red at 20c192d: 7 failed / 502 passed (all failures the new tests). HEAD: **509 passed / 5 skipped**, ruff clean on changed files (known pre-existing E501 only). Orchestrator re-ran both.
- Merged main tree: pytest 509 passed / 5 skipped.
- Stamping tests drive the REAL `/tools/call` endpoint and assert on-disk journal filenames (`req-*.json` present, `shim-STALE.json` absent).

## Review

Verdict: approve (no blocking findings). Rubric: correctness 8, spec_compliance 10, safety 9, clarity 10.

Advisories (recorded, not fixed):
- **Late-tool-call window at WS teardown**: `aclose` clears the active id and interrupts codex best-effort, but a `/tools/call` already dispatched can land after — falling back to the FIRST-chat shim id, attributing a stray write to the first-chat journal. Confined to the departing single session; never mis-stamps a future request (chat is gated out of executing). Candidate one-line comment.
- `reset` rejected in `plan_review` broadens the task wording (executing-only) — no dead-end (feedback/approve/cancel all exit; cancel → idle → reset works); error text "in-flight turn" slightly inaccurate for plan_review.
- `_resume_turn_text` section markers share the first-turn trust boundary (user-controlled text, same BYO account) — no escalation.

## Harness Sync

- features/05_agent-core.md += `server/eud_agent/engine.py` (BOUND — was missing since EUD-056; gap closed)
- No manifest changes.

## Notes

- EUD-062 isolation byte-for-byte preserved (reviewer-verified); the runner docstring premise about per-chat threads was rewritten to the continuity semantics.
- EUD-061 live E2E should additionally verify: a second chat referencing the first ("아까 말한 거") is understood; reset starts a clean conversation.
- Worker note: worktree branch started 21 commits behind main; fast-forwarded (strict ancestor) before working.
