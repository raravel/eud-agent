---
task_id: EUD-073-4693
completed_at: 2026-06-05T21:05:00
duration_minutes: 15
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
  input: 30000
  output: 6000
cost_usd: 1.10
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-direct
---

## Summary

Live-E2E defect: the plan card rendered EMPTY although the propose_plan tool
round-trip carried the markdown (user pasted the exact request/response). Root
cause: `_dig_markdown` only understood bare `{"markdown"}` dicts / JSON strings
/ `.markdown` attrs — but LIVE shapes are (a) an MCP result object whose
content blocks are plain dicts with a JSON-string text payload
(`{"ends_turn": true, "markdown": "..."}`), and (b) call arguments shim-wrapped
as `{"args": {"markdown": ...}}`. Every branch fell through → `""` → the engine
still ended the turn as a plan (the `is not None` check passes for `""`) with
empty markdown.

**Fix**: `_dig_markdown` now (1) extracts MCP result objects via
`_tool_result_text` (dict + typed blocks) and re-digs the JSON text, and (2)
unwraps the shim's `args` nesting. Both live shapes pinned by tests.

## Changes

`server/eud_agent/agent_runner.py` (_dig_markdown),
`server/tests/test_agent_flow.py` (+2 tests with the verbatim live shapes).

## Verification

Verify-first: both tests red, green after. Full server suite 534 passed; ruff
clean; panel static contract 18 passed.

## Harness Sync

No spec change (extraction is an implementation detail of the documented
propose_plan flow). No manifest changes.

## Notes

- The same dict-block extraction gap bit EUD-068's result field earlier today
  (fixed in EUD-072); this closes the remaining consumer (_plan_markdown_from).
