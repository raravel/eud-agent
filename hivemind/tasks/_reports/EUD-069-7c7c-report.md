---
task_id: EUD-069-7c7c
completed_at: 2026-06-05T19:55:00
duration_minutes: 40
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 70000
  output: 16000
cost_usd: 2.80
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-playwright-e2e
---

## Summary

Live-E2E defects 3-4 ("계획을 작성하면 UI가 깨져서 승인할 수 없음" / "승인하면 그
다음 출력이 깨짐 — 도구는 계획 영역에, 메시지는 메시지창에"). Root cause
reproduced with the Playwright WS-stub harness replaying the EXACT live event
sequence (14 tool rounds + plan): the AgentStream rendered as a FIXED band
between the conversation and the input inside `h-screen flex-col`, with no
overflow handling — `min-height:auto` made it unshrinkable, so 14 tool rows
consumed 744px, crushing the log to 0px and the plan section to 33px (content
378px), pushing the 승인 button off-viewport (top=1112 > 900) and overflowing
body to 959px. After approval the same band collected the NEW turn's tool rows
directly above the still-visible plan card — the "tools in the plan area" split.

**Fix** (two parts):
1. **Inline placement**: ConversationLog now receives the per-turn buffers and
   renders the AgentStream (Reasoning + Tool rows) and the live AgentAnswer
   bubble INLINE at the end of the Conversation scroll content; the App-level
   fixed band is removed. Everything scrolls together (stick-to-bottom keeps
   the latest in view).
2. **Turn-end archiving**: when a turn ends (answer/plan/changeset/error), the
   tool rows archive into the log as a compact entry CARRYING the rows
   (`LogEntry.tools` → "도구 호출 n건 — name×k" + expandable ToolList cards)
   and the live buffer clears — stale rows can no longer occupy the next phase.
   Order: tools archive BEFORE the F2 prose archive (history reads tools→prose).

## Changes

`panel/src/state/store.ts` (LogEntry.tools, archiveTurnTools, 4 turn-end call
sites; +6 tests), `panel/src/components/ConversationLog.tsx` (turn prop, inline
stream, archived-tools rendering; +3 tests), `panel/src/components/AgentStream.tsx`
(ToolList extraction, padding fix), `panel/src/App.tsx` (band removed),
features/06 doc (UI layout contract).

## Verification

Verify-first: 6 store tests + 3 ConversationLog tests red, green after. Full
panel vitest 215 passed; build green. **Playwright replay of the live scenario
against the rebuilt dist** (same viewport 820x900):

| metric | before (live defect) | after |
|---|---|---|
| conversation log height | 0px | 358px (718px post-answer) |
| plan section height | 33px (content 378px) | 360px, content visible |
| 승인 button | top=1112, OFF-viewport | inViewport: true (clicked for real) |
| body overflow | 959px > 900 | none |
| tool rows at plan time | 14 live rows, 744px | archived line "도구 호출 14건 — project_status, dat_get×12, propose_plan" |
| post-approve apply turn | tools in a separate band above the plan | tool rows + live answer INLINE in the log; plan card intact (360px) |

Screenshot: `eud-069-fixed-layout.png` (evidence, not committed).

## Harness Sync

features/06 gained the "Inline stream placement" behavior bullet. No manifest
changes; ToolList is an internal export of AgentStream.tsx (already bound).

## Notes

- Reasoning stays in the live buffer after turn end (collapsed, re-expandable)
  and clears on the next turn — GPT-style.
- The Playwright WS-stub harness was reused verbatim from EUD-066; it remains
  the cheapest pre-editor E2E gate.
