---
created: '2026-06-12'
depends_on:
- EUD-155-2da4
id: EUD-156-ade9
parent: EUD-151-e190
priority: high
scope:
- ci/build_rag_index.rs
status: pending
title: 'build_rag_index: source to tier_level + v2 write'
type: task
updated: '2026-06-12'
---

## Description
In `ci/build_rag_index.rs`, derive `tier_level` from `JsonlRow.source` per the 4-level mapping and
write the v2 format byte-identical to `rag.rs` (T2). The original board filename is only available at
build time, so the mapping must key off `source` before it is collapsed into the `[title](url)` label.

Mapping: `eud_book`/`cafebook` -> 3 (official); `board_강좌팁`/`board_연구칼럼` -> 2 (lecture);
`board_유틸리티툴`/`board_Lua자료실`/`user_*` -> 1 (general); `board_질문답변` -> 0 (Q&A).

## Spec References
- [[features/17_rag-knowledge-tiering|17_rag-knowledge-tiering]] `../docs/features/17_rag-knowledge-tiering.md` — Source to tier mapping (4 levels)
- [[decisions/18_rag-tier-encoding|18_rag-tier-encoding]] `../docs/decisions/18_rag-tier-encoding.md`
- [[rules]] `../docs/rules.md` — DEFAULT_BATCH_SIZE=16 unchanged; UTF-8 no BOM

## Completion Criteria
- [ ] `source` -> `tier_level` mapping implemented for all four groups
- [ ] Builder emits VERSION=2 with tier_level matching the rag.rs layout
- [ ] Unit test covers each source group -> expected tier_level (incl. `user_*` prefix, board QnA -> 0)
- [ ] Unknown/unmapped source falls back to a documented default with a test
- [ ] cargo build + test (ci manifest) passes