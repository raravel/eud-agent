---
task_id: EUD-065-3247
completed_at: 2026-06-05T20:40:00
duration_minutes: 50
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 7
  spec_compliance: 8
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 540000
  output: 160000
cost_usd: 20.10
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

Panel chat surface rebuilt on vendored Vercel AI Elements + Streamdown (Decision 06; user decision 2026-06-05), closing the three reported rendering defects:

- **Vendored** (`panel/components/ai-elements/`, 9 components + shadcn card): conversation, loader, message, plan, reasoning, response, shimmer, tool, prompt-input. Adaptations (documented in file headers): message `ai`→local role; response = hand-built Streamdown wrapper; reasoning's `useControllableState` inlined; shimmer motion→CSS; tool local ToolState; prompt-input chat subset only (attachments/speech/command/etc. honestly not vendored — would drag nanoid/cmdk/ai uselessly). Deps: `streamdown`, `use-stick-to-bottom` — all npm-bundled, zero runtime CDN.
- **ConversationLog** → Conversation(auto-scroll)+Message: agent answers PROMINENT foreground Message/Response (Streamdown) — styling inversion fixed; user bubbles; system/progress rows stay muted.
- **Live streaming** (store `state.turn` buffers, reset per turn): `delta` text → live prominent AgentAnswer bubble (mounts only during thinking; final `answer{}` archives to the log); `reasoning` text → Reasoning block (dim, auto-open while streaming, auto-collapse when the answer starts, manually re-expandable after — review F1); `tool_call`/`tool_result` → Tool rows by name + 도구 호출 n건.
- **No-raw-kind-leak contract**: store swallows internal kinds (delta/answer/token_usage/turn_done/item_*/event never reach the log); leak tests assert against rendered DOM + log text.
- **PlanView** → Plan component + Streamdown (hand-rolled parseMarkdown removed); feedback/approve/pending gating preserved.
- **InstructionBox** → PromptInput + **[새 대화]** → `reset{}` + `resetSent()` (clears log/plan/changeset/buffers; undecided-changeset discard surfaces a warn notice — review F3); disabled mid-turn.
- **ChangesetView** restyled (Card/Badge); diff body byte-intact (server unified diff, no Monaco DiffEditor); lib/changeset helpers + all decision flows preserved.
- **protocol.ts**: `reset{}` added; agent_event kinds documented (open string).

Verify-first gate: Step A red committed first (3026aa0, 20 failed / 164 passed) — orchestrator re-ran red and HEAD.

## Changes

28 files +5390/-864 (incl. package-lock 3743): 9 vendored components + card, App/AgentStream/AgentAnswer(new)/ConversationLog/InstructionBox/PlanView/ChangesetView, store.ts (turn buffers + resetSent + archiveTurnAnswer), protocol.ts, test files, test_panel_static.py.

## Verification

- Red at 3026aa0: 20 failed / 164 passed. Final HEAD (4476c8e): vitest **197 passed** (15 files), `npm run build` green (Streamdown/shiki chunks local), pytest test_panel_static **18 passed** (incl. the new dist-chunk CDN-host scan). Orchestrator re-ran all three at red, post-implementation, and post-review-fix.
- Built dist external-origin scan: index.html attrs AND `dist/assets/*.js` CDN hostnames — clean.

## Review

Review round 1 — initial rubric: correctness 7, spec 8, safety 9, clarity 9; no blocking findings, but the orchestrator required a fix round because two advisories contradicted the user's explicit "GPT-style" intent. Fixed (commit 4476c8e):

- **F1**: permanently-controlled `open` made the collapsed Reasoning block un-re-expandable (dead toggle). Fixed with a per-turn user-override state + `onOpenChange`; auto-open/auto-collapse preserved; re-expand test added.
- **F2**: prose streamed via `delta` was silently discarded when a turn ended in `plan{}`/`changeset{}`/`error{}` (engine emits exactly one turn-end message). Fixed with store-side `archiveTurnAnswer()` at those transitions; `answer{}` stays authoritative (no double-log); 4 tests.
- **F3**: [새 대화] over an undecided changeset discarded the review UI silently — now logs "미결정 변경사항은 자동 적용 처리되었습니다." as the fresh log's first entry (server default-accepts per features/05); 3 tests.
- **F4**: the dist external-origin scan only inspected index.html while claiming broader coverage — extended to scan `dist/assets/*.js` for CDN hostnames (no false positives on W3C namespaces etc.).

Safety verified by the reviewer at source level: streamdown@2.5.0 applies rehype-sanitize + rehype-harden by default (vendored wrappers don't disable it) — the v2 zero-HTML-injection guarantee is preserved; no dangerouslySetInnerHTML/eval anywhere; no runtime CDN in built chunks (manual deep scan).

## Harness Sync

- features/06 `## Implementation` already lists the component paths + `panel/components/ai-elements/` + streamdown (spec amended before the task — no drift).
- Dep binding: `streamdown` + `use-stick-to-bottom` recorded here; tech-stack.md has no panel dep section to append to (panel deps tracked via package.json + features/06). No removed deps.

## Notes

- EUD-046/EUD-047 surface: the panel now consumes EUD-063's `reasoning`/`delta` kinds and EUD-064's `reset{}` end-to-end.
- Live E2E (EUD-061) must visually confirm: reasoning dim/collapsible + re-expandable, answer prominent + streamed live, no raw kind strings, [새 대화] resets the conversation (server thread drop).
- The >500kB chunk-size advisory from vite is expected (shiki); assets are local.
