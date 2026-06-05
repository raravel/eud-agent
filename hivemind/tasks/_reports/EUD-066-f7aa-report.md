---
task_id: EUD-066-f7aa
completed_at: 2026-06-05T21:35:00
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
  input: 40000
  output: 12000
cost_usd: 1.60
profile: quality
models:
  executor: orchestrator-direct
  reviewer: orchestrator-playwright-e2e
---

## Summary

Live UI defect (user report: "깨지고 인터렉션도 안 돼"): the prompt input rendered as a ~24px-wide textarea with the placeholder flowing VERTICALLY, overflowing 600+px above the input bar.

**Root cause** (diagnosed live via Playwright against the built dist): `InstructionBox.tsx` nested `PromptInputFooter` INSIDE `PromptInputBody`. The body is a `display: contents` div, and the InputGroup's column layout depends on CSS `:has(> ...)` DIRECT-child selectors (`has-[>[data-align=block-end]]:flex-col` / `:h-auto` in ui/input-group.tsx). `:has(>)` is DOM-structural — `display: contents` does not flatten the DOM for selector matching — so neither variant matched: the group stayed a fixed-height (36px) flex ROW where the textarea (flex-basis 0) collapsed to 24px and overflowed vertically (`field-sizing: content`). Hypothesis was confirmed live BEFORE coding by moving the footer node in the browser: layout healed instantly (column / 116px / textarea 1223×64).

**Fix**: footer moved out to a SIBLING of the body (direct child of the InputGroup), with a comment documenting the structural contract. jsdom cannot do layout, which is why 198 unit tests missed this — a DOM-STRUCTURE test now pins what the selectors require (direct-child addon present). Note: an initial `:scope > ...` selector version of the test was a false-red (jsdom selector-engine gap); replaced with direct children iteration and re-validated red-on-buggy/green-on-fixed via git stash.

## Changes

`panel/src/components/InstructionBox.tsx` (footer relocation + contract comment), `panel/src/components/InstructionBox.test.tsx` (+structural test).

## Verification

- Verify-first: structural test FAILS on the buggy composition (stash-validated), passes on the fix. Full vitest **198 passed** (15 files); `npm run build` green.
- **Playwright live E2E against the built dist** (static serve + WebSocket stub injecting server messages):
  - Layout healed (screenshot-verified): horizontal placeholder, full-width textarea, 새 대화/전송 in the footer row.
  - Full interaction pass: send gating (enabled after status+list), chat{} sent with typed text, reasoning deltas → dim collapsible block, auto-collapse on answer start, **manual re-expand/re-collapse works** (EUD-065 F1 confirmed live), tool_call/tool_result → Tool rows + 도구 호출 1건, delta streaming → prominent live answer, no raw kind leak (token_usage/turn_done injected — nothing surfaced), answer{} archived once (no duplication), plan{} → Plan card with Streamdown headings + 승인/수정 buttons (plan_approve{} sent on click), [새 대화] → reset{} sent + log cleared.
  - Markdown render confirmed: Streamdown emits `<span data-streamdown="strong" class="font-semibold">` for bold (not `<strong>`) and styled `<code>` — both visually correct.

## Review

Orchestrator-direct fix (2-line structural move, explicit cause, bounded scope) — no worker review round; the Playwright interaction pass above served as the adversarial verification. Findings during the pass, both resolved as non-defects: hasProject gates on the `list` reply (stub harness gap, correct product behavior); bold renders as a styled span (probe error).

## Harness Sync

harness sync: no-op — InstructionBox.tsx already in features/06 `## Implementation`; no manifest changes. Pass.

## Notes

- Same-class risk: any FUTURE ai-elements composition must keep `InputGroup`-level addons as DIRECT children (the `:has(>)` contract). The vendored prompt-input.tsx now carries this in the InstructionBox comment; consider a lesson if it recurs.
- The WebSocket-stub Playwright technique (static dist + injected server messages) is a cheap pre-editor E2E harness — reusable before EUD-061 live runs.
