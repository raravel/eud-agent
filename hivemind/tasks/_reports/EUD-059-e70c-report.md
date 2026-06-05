---
task_id: EUD-059-e70c
completed_at: 2026-06-05T15:58:13
duration_minutes: 30
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
review_scores:
  correctness: 7
  spec_compliance: 9
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 296012
  output: 74003
cost_usd: 9.99
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary

The panel v2 review UI (features/06 UI layout + Behaviors):

- **`ChangesetView.tsx`** (new) — renders server-grouped changeset items: dat groups per objId (header + property old→new rows), files by kind (created → truncated preview; modified → SERVER unified diff rendered line-by-line as TEXT with +/- coloring — no Monaco DiffEditor, no dangerouslySetInnerHTML; deleted → name row; `lib/diff` + `lib/truncate` 1 MiB rules reused), settings/plugins/main old→new rows. Per-item ✓적용/✗되돌리기 + bulk 전체 적용 유지/전체 되돌리기 → `changeset_decision{decision, ids}` (single ids vs literal "all"); row states from store decisions (적용 유지/되돌림/되돌리기 실패 inline); buttons disabled while a decision is in flight.
- **`lib/changeset.ts`** (new) — pure helpers handling the server's id shape: dat groups carry ids ONLY on `properties[].id` (no item-level id); `itemIds`/`itemState`/`itemKey` (stable key: item id or joined property ids).
- **`AgentStream.tsx`** (new) — live `도구 호출 n건 · 현재: …` activity line from `agent_event`s; collapses to a summary row when the turn leaves thinking; events reset per turn (no unbounded growth).
- **`Header.tsx`** — 연결 중 → 연결됨 → 재연결 중 + RAG pill with elapsed seconds while loading (`formatElapsed` in `lib/progress.ts`; App tracks the rag_warmup start ts with a cleaned-up 1s interval; Header stays pure).
- **`App.tsx`** — real components wired (PlanView remains the next task's placeholder).
- **`test_panel_static.py`** — TargetPicker/ApplyBar ABSENCE + ChangesetView/AgentStream presence guards; no-CDN/no-BOM contract intact (built-dist external-origin scan ran).

Verify-first gate: Step A failing suites committed first (component modules absent, header/progress assertions red).

## Changes

Implementation: `ChangesetView.tsx`/`AgentStream.tsx` (new), `Header.tsx`/`App.tsx`/`lib/progress.ts` (modified), `lib/changeset.ts` (new, scope-add), `panel/src/state/store.ts` (review-round fix, scope-add), `server/tests/test_panel_static.py`. Tests: changeset.test.ts/ChangesetView.test.tsx/AgentStream.test.tsx (new), Header.test.tsx/progress.test.ts/store.test.ts (updated).

## Verification

- Step A red (modules absent / wording+formatElapsed failures); worker post-review: vitest **158/158** (13 files), build green, pytest 495/4 — orchestrator re-ran all three.
- Merged main tree: panel build green; server **495 passed / 4 skipped** (panel static 17/17).

## Review

Review round 1 — initial rubric: correctness 7 with a blocking finding, spec 9, safety 10, clarity 9. Fixed:

- **F1 (blocking)**: ChangesetView keyed dat groups on `item.id`, which the server never sends for dat groups (ids live on `properties[].id`) — with 2+ dat groups (multiple edited units, the common case): duplicate `undefined` React keys → unstable row identity/badge mis-rendering; testid collisions. The test fixtures had masked it by injecting a fake id production never supplies. Fixed with `itemKey` (stable derived key) + representative id-less fixtures + a multi-group independence test.
- **F2 (cross-task, same root cause — store.ts scope-added)**: EUD-058's `isChangesetFullyDecided` checked `decisions[it.id]` → a changeset containing ANY dat group could never reach "fully decided": rollback_result never left changeset_review and reconnect re-opened review forever — broken primary flow for the most common changeset content. Fixed by deriving completion from the shared `itemIds` helper (a dat group completes when all property ids are decided); the bulk-accept undecided-id derivation had the same bug, fixed the same way. +5 store tests (incl. reconnect persistence both directions).

Advisory (left as-is, within spec wording): `formatElapsed` renders flat seconds (75초) with no minute rollover.

## Incident

### What broke
- dat-group id shape mismatch in two layers: the view keyed rows on a field the server never sends; the store could never complete a dat-bearing changeset.

### Why
- journal.py's changeset format puts dat ids on `properties[].id` only; both panel layers assumed item-level ids, and the fixtures injected fake ids that hid it from the suites.

### What fixed it
- Review round 1 (commit fb6e846): shared `itemKey`/`itemIds` helpers used by BOTH the view and the store; representative fixtures; +10 tests (158 total).

## Harness Sync

harness sync: no-op for bindings — components/lib paths covered by features/06 `## Implementation`; no manifest changes. Contract-drift guard: additive UI + the spec-mandated absence guards; pass.

## Notes

- EUD-060 (deferred this round on App.tsx overlap) presumably owns PlanView + the remaining features/06 surface; EUD-061 follows.
- The store's changeset-completion semantics now live in `lib/changeset.ts` — PlanView/later tasks should reuse the same helpers rather than re-deriving id shapes.
