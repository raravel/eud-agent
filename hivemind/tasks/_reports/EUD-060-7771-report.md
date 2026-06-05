---
task_id: EUD-060-7771
completed_at: 2026-06-05T16:20:00
duration_minutes: 20
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
  input: 130468
  output: 32617
cost_usd: 4.40
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

PlanView + feedback iteration UI (features/06 Behaviors → Plan review), closing the EUD-046 panel-v2 story:

- **`PlanView.tsx`** (new) — markdown plan card rendered with a minimal SAFE line-based renderer (ATX headings, ul/ol lists, code fences, paragraphs; every string rendered as React children = auto-escaped; NO dangerouslySetInnerHTML, NO new runtime deps, inline emphasis intentionally literal — zero HTML-injection surface, reviewer-verified incl. a genuine no-script-node test). 피드백 textarea → `plan_feedback{text}` (empty-guard, clears after a confirmed send); [수정요청]/[승인] (승인 → `plan_approve{}`); `pending` disables controls when the phase leaves plan_review. Korean labels.
- **`App.tsx`** — placeholder replaced; handlers send via the WS client and mutate the store ONLY on a confirmed send; plan cards archived into the conversation log on arrival / supersession (higher revision, prior card read before overwrite) / approval, using existing log kinds (500-cap bounded).
- Visibility judgment (reviewer-endorsed): the card persists with disabled controls during the feedback/approve turn — safe because every other path into `thinking` nulls the plan first.

Verify-first gate: Step A failing suite committed first (module absent red).

## Changes

`panel/src/components/PlanView.tsx` (new, 249), `panel/src/components/PlanView.test.tsx` (new, 9 tests), `panel/src/App.tsx` (+68/-12).

## Verification

- Step A red; worker: vitest 167/167 (14 files), build green — orchestrator re-ran both in the worktree.
- Merged main tree: panel build green; server suite unaffected (495 passed / 4 skipped).

## Review

Verdict: approve. Rubric: correctness 9, spec_compliance 9, safety 10, clarity 9. No blocking findings.

Advisories (recorded, not fixed):
- `parseMarkdown` splits on `\n` only — CRLF input would leave cosmetic trailing `\r` in rendered text (plan markdown arrives from codex over JSON WS, practically always `\n`).
- App-level archival logic untested — no App test harness exists in the repo (pre-existing gap).
- PlanView shown during `thinking` (disabled) is a defensible extension beyond the literal "when plan{} active" wording.

## Harness Sync

harness sync: no-op — PlanView/App listed in features/06 `## Implementation`; no manifest changes; additive. Pass.

## Notes

- **EUD-046 (panel v2) story auto-completed** with this merge: WS v2 protocol/store → ChangesetView/AgentStream/Header → PlanView.
- Three stories now closed this session: EUD-044 (bridge v2 surface), EUD-045 (agent core), EUD-046 (panel v2). Remaining: whatever EUD-043's last story (EUD-047?) holds — likely E2E/docs.
