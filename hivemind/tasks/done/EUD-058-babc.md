---
completed_at: '2026-06-05T15:35:51.931410'
created: '2026-06-05'
depends_on:
- EUD-056-5ca7
id: EUD-058-babc
parent: EUD-046-f2c8
priority: high
scope:
- panel/src/ws/protocol.ts
- panel/src/ws/client.ts
- panel/src/state/store.ts
- panel/src/state/store.test.ts
- panel/src/App.tsx
- panel/src/components/ConversationLog.tsx
- panel/src/components/InstructionBox.tsx
- panel/src/components/ApplyBar.tsx
- panel/src/components/TargetPicker.tsx
- panel/src/components/ReviewTabs.tsx
- panel/src\components\ApplyBar.test.tsx
- panel/src\components\ConversationLog.test.tsx
- panel/src\components\DiagnosticsStrip.test.tsx
- panel/src\components\Header.test.tsx
- panel/src\components\InstructionBox.test.tsx
- panel/src\components\ReviewTabs.test.tsx
- panel/src\components\TargetPicker.test.tsx
- panel/src\lib\diff.test.ts
- panel/src\lib\progress.test.ts
- panel/src\lib\truncate.test.ts
- panel/src\state\store.test.ts
- panel/src\ws\client.test.ts
- panel/src\ws\protocol.test.ts
status: done
title: Panel WS v2 protocol + store state machine rework
type: task
updated: '2026-06-05'
---

## Description
Panel WS v2 + store rework per spec: protocol.ts/client.ts gain chat/plan_feedback/plan_approve/changeset_decision/cancel and agent_event/answer/plan/changeset/rollback_result; v1 instruct/apply/code/applied removed; store implements the v2 state machine (ready/thinking/plan_review/changeset_review) with reconnect-safe resets and changeset persistence; send gating becomes connected AND hasProject AND not-busy (settable-target gate removed).

## Spec References
- [[features/06_changeset-review-panel|06_changeset-review-panel]] `../docs/features/06_changeset-review-panel.md` — State machine / Behaviors

## Completion Criteria
- [ ] vitest: every transition incl. reconnect mid-thinking (notice + ready) and changeset still reviewable after reconnect
- [ ] v1 message types absent from protocol.ts (grep test)
- [ ] Send gating tests: no-project blocks, busy blocks, settable-target NOT required
- [ ] npm --prefix panel run build green