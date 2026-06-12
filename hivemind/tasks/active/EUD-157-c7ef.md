---
created: '2026-06-12'
depends_on:
- EUD-155-2da4
id: EUD-157-c7ef
parent: EUD-151-e190
priority: high
scope:
- src-tauri/src/rag.rs
status: pending
title: Weighted rank() with TIER_WEIGHT table
type: task
updated: '2026-06-12'
---

## Description
Make `rank()` tier-aware: `score = cosine(query, entry) * TIER_WEIGHT[entry.tier_level]`, where
`TIER_WEIGHT` is a 4-element code constant in a narrow band near 1.0 (e.g. `[1.00, 1.05, 1.10, 1.15]`)
so tier nudges but never dominates the cosine signal. Tie-break stays lower-`id`; `MAX_TOP_K` clamp
unchanged. Pin the weights with a test that documents the chosen band.

## Spec References
- [[features/17_rag-knowledge-tiering|17_rag-knowledge-tiering]] `../docs/features/17_rag-knowledge-tiering.md` — Weighted ranking
- [[decisions/18_rag-tier-encoding|18_rag-tier-encoding]] `../docs/decisions/18_rag-tier-encoding.md` — weight is a code constant

## Completion Criteria
- [ ] `TIER_WEIGHT` 4-element constant defined near 1.0
- [ ] `rank()` multiplies cosine by `TIER_WEIGHT[tier_level]`
- [ ] Test pins the weights and asserts tier influences order without overpowering cosine (documented case)
- [ ] Tie-break lower-`id` and `MAX_TOP_K` clamp preserved
- [ ] cargo test passes