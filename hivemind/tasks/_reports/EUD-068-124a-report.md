---
task_id: EUD-068-124a
completed_at: 2026-06-05T19:55:00
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
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 50000
  output: 14000
cost_usd: 2.20
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-playwright-e2e
---

## Summary

Live-E2E defect 2 ("도구를 호출할 때 요청한 값, 받은 값 필요"). The SDK delivers
`McpToolCallThreadItem.arguments` on item/started and `result`/`status`/`error`
on item/completed (official app-server protocol fields), but `_classify_event`
forwarded only the bare tool NAME both ways — the live run's 4 rounds of
arg-shape retries (`table/id/field` → `dat/objId/param`) rendered as identical
bare `dat_get` rows.

**Fix**: server extracts args (`_tool_args_text`: JSON-string passthrough /
compact-JSON dump, truncated at `TOOL_DATA_MAX_CHARS=4000` with an explicit
…(잘림) marker) and result (`_tool_result_data`: joined MCP content text, error
message on failure, status value). They ride a new OPTIONAL `data` field on
`agent_event`. Panel: `AgentTool` gains `args` + a `failed` state (non-completed
status); Tool cards render 요청/결과 blocks inside the expandable content; the
vendored tool.tsx gains a 실패 badge (XCircle/destructive).

## Changes

`server/eud_agent/agent_runner.py` (+5 tests in test_agent_flow.py),
`panel/src/ws/protocol.ts`, `panel/src/state/store.ts` (+4 tests),
`panel/src/App.tsx`, `panel/src/components/AgentStream.tsx` (+2 tests),
`panel/components/ai-elements/tool.tsx` (failed badge), features/05+06 docs.

## Verification

Verify-first: 5 server tests red (no event_data), 3 panel store tests red.
Green after. Full suites: server 528 passed, panel vitest 215 passed, build
green. Playwright WS-stub pass: expanded dat_set card shows 요청 JSON + 결과
text; a `status:"failed"` result renders the 실패 badge (screenshot-verified).

## Harness Sync

features/05 `agent_event{kind, detail, data?}` amended; features/06 Agent-stream
bullet amended. No new deps.

## Notes

- Truncation is server-side (panel render safety); the 1 MiB lib/truncate stays
  for changeset previews only.
- Legacy `agent_event` without `data` keeps the old done-flip behavior (pinned
  by a regression test).
