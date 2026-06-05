---
task_id: EUD-067-961b
completed_at: 2026-06-05T19:55:00
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
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 60000
  output: 10000
cost_usd: 2.00
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-probe-verified
---

## Summary

Live-E2E defect 1 ("reasoning 텍스트가 안 나와서 작업중인지 피드백이 없음"). Root
cause established BEFORE coding via codex session rollouts + 3 SDK probe turns:

- **No reasoning ever streamed** because codex requests `reasoning.summary` from
  the API only when the MODEL-FAMILY metadata marks summaries as supported, and
  gpt-5.5's family ships with it OFF. The live thread's 7 reasoning items all had
  `summary=[]` (encrypted content only); a probe with
  `model_supports_reasoning_summaries=true` produced **79
  item/reasoning/summaryTextDelta notifications** on one turn (zero without).
  The EUD-063 classifier and the panel pipeline were correct — starved of input.
- **Silence amplifier**: the SDK default `ApprovalMode.auto_review` spawned a
  HIDDEN guardian reviewer thread running a full model review turn per MCP tool
  call (21 review turns in the live E2E rollout `rollout-...18-46-24`) — 10-25s
  silent gaps between tool calls and ~2x token burn.

**Fix** (`server/eud_agent/agent_runner.py`): launch-level
`REASONING_VISIBILITY_OVERRIDES` (`model_supports_reasoning_summaries=true` +
`model_reasoning_summary="detailed"`) composed into the config_overrides BEFORE
`extra_overrides` (injection can still flip them); new `_thread_start_kwargs`
passes `ApprovalMode.deny_all` on thread_start (no guardian — the server is
already the policy layer).

## Changes

`server/eud_agent/agent_runner.py`, `server/tests/test_agent_flow.py`
(+`test_reasoning_visibility_overrides_present`,
`test_thread_start_kwargs_disable_guardian_reviewer`), features/05 doc.

## Verification

Verify-first: both tests red (AttributeError / missing override), green after.
Full server suite 528 passed / 4 skipped; ruff clean; selfcheck OK. Live probe
evidence (79 summary deltas with the flag) recorded above; the actual panel
display path was re-verified via the Playwright WS-stub harness (reasoning
deltas → dim collapsible block).

## Harness Sync

features/05 "WS protocol v2" gained the Reasoning-visibility + no-guardian
contract paragraphs. No manifest changes.

## Notes

- The internet finding that unlocked this: codex sends `reasoning.summary` only
  per model-family support metadata (openai/codex PR #3171 lineage; issue
  #16801 shows the same config family).
- `deny_all` means approval escalations are auto-denied (never asked). The agent
  uses only eud-tools MCP calls, which do not require approvals.
