---
task_id: EUD-056-5ca7
completed_at: 2026-06-05T15:10:33
duration_minutes: 45
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 8
  spec_compliance: 8
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 486754
  output: 121689
cost_usd: 16.43
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

The v2 engine (features/05 Engine + Triage + WS protocol v2):

- **`agent_runner.py`** (new) — `AgentRunner` ABC + `CodexSDKRunner` over the official SDK (shapes reused verbatim from the EUD-053 spike, not re-researched). The sync SDK turn runs in `asyncio.to_thread`; stream events are forwarded to the WS loop via `run_coroutine_threadsafe` (loop captured at turn start, 10s send backstop). Per-thread MCP injection of `eud_agent.mcp_shim` with `EUD_DATA_DIR` + `EUD_REQUEST_ID`; thread-id retained per panel session (resume for plan_feedback/plan_approve); `propose_plan` detected in the stream ends the turn; `cancel()` → `TurnHandle.interrupt()`.
- **`engine.py`** (new, scope-add) — `AgentEngine`: the small deterministic state machine (`idle → triage → answer | apply | plan_review* → executing → changeset_review → idle`), dict-dispatch on WS type; turns run as background tasks so the receive loop stays free for `cancel`/disconnect; `build_system_prompt` (tool catalog + project state + RAG top-k + triage rules, each section degrading on failure); `parse_status` moved verbatim from the deleted orchestrator. A new chat from `changeset_review` FINALIZES the prior request's journal first (undecided → accepted, archived with a note — features/05 line 45).
- **`app.py`** — WS v2 routing replaces v1 (`chat`/`plan_feedback`/`plan_approve`/`changeset_decision`/`cancel`/`status`/`list` in; `agent_event`/`answer`/`plan`/`changeset`/`rollback_result`/`error`/`status`/`progress` out). v1 `instruct`/`apply`/`code`/`applied` REMOVED (unknown-type error, no compat shim). Injectable `runner_factory` (FakeRunner CI testing); ToolLayer wired with the real Journal factory. **Token + Origin security posture byte-identical** (reviewer-verified against base).
- **`orchestrator.py` DELETED** (spec-mandated retirement); `codex_client.py`/`lsp_gate.py` left in place off the main flow.
- **request_id wiring traced end-to-end** (reviewer): engine mints `req-<uuid8>` → runner env-injects `EUD_REQUEST_ID` → shim forwards it to `/tools/call` → ToolLayer gate/journal key on the same id → engine approve/changeset use the same id.

Verify-first gate: Step A failing suite committed first (8 failed / 1 skipped red).

## Changes

- `server/eud_agent/agent_runner.py` (new, 370), `server/eud_agent/engine.py` (new, ~450), `server/eud_agent/app.py` (~105 changed), `server/eud_agent/orchestrator.py` (deleted, 293)
- `server/tests/test_agent_flow.py` (new, ~620 incl. env-flagged real-codex smoke `EUD_REAL_CODEX_SMOKE=1`, skipped in CI), `server/tests/test_integration_ws.py` (migrated to v2 — real file-IPC fake bridge asserts an actual inverse `SETDAT` .cmd on reject), `server/tests/test_orchestrator.py` (repurposed to engine units), `server/tests/test_lsp_gate.py` (2 dead-import tests removed; `diagnose()` coverage intact)
- Scope-adds (orchestrator-approved, no in-flight peers): `engine.py`, `test_lsp_gate.py`.

## Verification

- Step A: red (TypeError runner_factory / unknown-type asserts; 8 failed, 1 skipped).
- Worker worktree: ruff clean; 469 passed / 5 skipped after review round.
- Merged main tree: ruff clean; **470 passed / 4 skipped**; `python -m eud_agent --selfcheck` green (codex shim, RAG DB, panel/dist all resolve).
- Real-codex smoke NOT run (spends BYO tokens) — runnable manually via `EUD_REAL_CODEX_SMOKE=1`.

## Review

Verdict: approve. Rubric: correctness 8, spec_compliance 8, safety 10, clarity 9. No blocking findings. Review round 1 fixed:
- features/05 line 45 gap — undecided changeset now finalized (default-accept + archive-with-note) when a new chat leaves `changeset_review`; 2 new tests (e2e with real Journal + unit with FakeJournal).
- Real-codex smoke lambda arity corrected.

Advisories (recorded, live-E2E items):
- `TurnHandle.interrupt()` cancel-unblock is unverified against the real SDK (the spike never called it) — confirm during the user-assisted editor session before relying on cancel in the field.
- `lsp_gate.diagnose()` is now unreachable from the main flow (its only caller was the v1 orchestrator) — left in place per the surgical-changes rule; a future task may re-wire advisory diagnostics into the v2 flow or retire the module.

## Harness Sync

harness sync: partial no-op — `agent_runner.py`/`app.py` already in features/05 `## Implementation`; `engine.py` is NEW and not yet listed (features/05 names the state machine but attributes routing to app.py). Binding appended via the report note below; the v1 docs (architecture.md runtime flow + WS table, features/02 orchestrator references) now describe a RETIRED flow — the accumulated docs-refresh debt (flagged since EUD-049) is now pressing: architecture.md still documents v1 instruct/apply and the v6-only IPC table. Recommend a dedicated docs task before the panel v2 work begins. Contract-drift guard: orchestrator.py deletion and v1 message removal are features/05's explicit mandate ("orchestrator.py v1 flow retired", "v1 messages are REMOVED") — spec-mandated, not drift. Pass.

## Notes

- `engine.py` binding: `server/eud_agent/engine.py` — AgentEngine state machine + system-prompt builder (belongs under features/05 Implementation; appended manually here due to the misfiring hv feedback dedup gate).
- Next per the story: EUD-057+ (likely euddraft fallback runner edd_runner.py, panel v2, docs refresh).
