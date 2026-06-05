---
task_id: EUD-074-d226
completed_at: 2026-06-05T21:05:00
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
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 45000
  output: 11000
cost_usd: 1.90
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-playwright-e2e
---

## Summary

User decision (live E2E 2026-06-05): plan-revision feedback must flow through
the MAIN prompt input ("원래 있던 프롬프트 모달에서 계속 입력하면서 계획 수정"),
not a separate embedded textarea — the PlanView feedback input is unnecessary.

**Fix**:
- `PlanView` drops the feedback textarea and the [수정요청] button (props lose
  `onFeedback`); the card keeps [승인] plus a hint pointing at the main input.
- Store: `plan_review` is no longer a send-gated busy phase (`BUSY_PHASES` =
  {thinking} only) — the main input stays enabled during plan review.
- App routes the send by phase: in `plan_review` the typed text goes out as
  `plan_feedback{text}` (+ the same log/`planFeedbackSent` flow the old button
  used); otherwise `chat{text}`.
- InstructionBox swaps its placeholder during plan_review:
  "계획 수정 피드백을 입력하세요 (승인은 계획 카드에서)".

## Changes

`panel/src/components/PlanView.tsx` (+test rewrite), `panel/src/state/store.ts`
(+1 test updated), `panel/src/App.tsx` (send routing; handlePlanFeedback
removed), `panel/src/components/InstructionBox.tsx` (+2 tests), features/06
(UI layout + Plan review behavior).

## Verification

Verify-first: 4 tests red (canSend, no-textarea, placeholder), green after.
Full panel vitest 215 passed; build green. **Playwright stub-E2E** against the
rebuilt dist: plan card shows content (with the EUD-073 server fix this is the
live markdown), NO feedback textarea / 수정요청 button, main input enabled with
the guidance placeholder, typing+send emits `{"type":"plan_feedback","text":…}`,
rev2 replaces the card, [승인] click emits `{"type":"plan_approve"}`.

## Harness Sync

features/06 UI-layout sketch + Plan review behavior bullet amended (EUD-074).
No manifest changes.

## Notes

- [새 대화] during plan_review still hits the server's "busy: cancel first"
  guard (pre-existing edge, unchanged).
