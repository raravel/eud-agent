---
completed_at: '2026-06-10T14:16:36.684861'
created: '2026-06-10'
depends_on: []
id: EUD-137-dcbc
parent: EUD-135-2f41
priority: medium
scope:
- ci/corpus
status: done
title: Vendor RAG corpus JSONL into ci/corpus
type: chore
updated: '2026-06-10'
---

## Description
Seed the in-repo corpus directory `ci/corpus/` with the current corpus JSONL (articles.jsonl,
eud_book.jsonl, cafebook.jsonl) so the CI embed step has its input without ECA. Plain-text JSONL,
committed to normal git (NOT LFS). This is the initial vendor; the Node/TS scraper (EUD-138) becomes
the sustainable refresh path. ToS/copyright of the scraped content is a user-acknowledged risk.

## Spec References
- [[features/16_rag-corpus-pipeline|16_rag-corpus-pipeline]] `../docs/features/16_rag-corpus-pipeline.md` — Corpus section
- [[decisions/15_in-house-rag-corpus|15_in-house-rag-corpus]] `../docs/decisions/15_in-house-rag-corpus.md`

## Completion Criteria
- [ ] `ci/corpus/articles.jsonl`, `ci/corpus/eud_book.jsonl`, `ci/corpus/cafebook.jsonl` present, UTF-8, one JSON object per line matching {title, content, url?, source}
- [ ] Files are tracked by normal git (NOT Git LFS); no `.gitattributes` LFS rule added for them
- [ ] `build_rag_index --corpus ci/corpus` (EUD-136) parses every line without error (validated by the orchestrator)