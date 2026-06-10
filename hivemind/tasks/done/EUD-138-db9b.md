---
completed_at: '2026-06-10T15:02:51.959283'
created: '2026-06-10'
depends_on: []
id: EUD-138-db9b
parent: EUD-135-2f41
priority: medium
scope:
- tools/scraper
- manifest:node
status: done
title: Node/TS Naver-Cafe scraper -> corpus JSONL (local, cookie)
type: task
updated: '2026-06-10'
---

## Description
New LOCAL-ONLY Node.js + TypeScript scraper under `tools/scraper/` that logs into Naver Cafe with a
supplied cookie and scrapes the EUD/eps boards into corpus JSONL rows, writing `ci/corpus/*.jsonl`
(atomic temp+rename). Never run in CI (cookie + ToS). Its own package.json + tsconfig (TypeScript
~5.9, matching panel); run via tsx/node. Pin scraping deps at implementation (e.g. an HTTP client +
HTML parser + cookie handling) and bind them to tech-stack.md.

## Spec References
- [[features/16_rag-corpus-pipeline|16_rag-corpus-pipeline]] `../docs/features/16_rag-corpus-pipeline.md` — Scraper section
- [[decisions/15_in-house-rag-corpus|15_in-house-rag-corpus]] `../docs/decisions/15_in-house-rag-corpus.md`

## Completion Criteria
- [ ] `tools/scraper/` is a self-contained Node/TS package (package.json, tsconfig.json, src/*.ts); `npx tsc --noEmit` passes
- [ ] Reads the Naver cookie from env/file (never committed); missing/expired cookie -> fails fast with a refresh hint, writes no partial corpus file
- [ ] Emits JSONL rows matching {title, content, url?, source} (the schema build_rag_index parses); writes atomically (tmp + rename); stable ordering for minimal diffs
- [ ] Polite scraping (throttle/delay, resumable); a dry-run / small-sample mode is documented in the README
- [ ] A unit test covers the JSONL row mapping (no live network); cookie/secret values are not logged