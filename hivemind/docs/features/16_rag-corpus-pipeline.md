# Feature 16: In-house RAG corpus pipeline (scrape -> corpus -> CI embed -> Release)

eud-agent owns the full RAG data pipeline. The corpus that the index embeds no longer lives in the
separate ECA repo; it is scraped, committed, embedded, and released entirely from this repo.

> Decision: see [[decisions/15_in-house-rag-corpus]] — supersedes the ECA-coupling aspect of
> [[decisions/10_rag-bruteforce-fastembed]] (the `.bin` format + Release distribution are unchanged).

## Pipeline

```mermaid
flowchart LR
    subgraph Local["LOCAL CORPUS REFRESH"]
        Cookie[(Naver login cookie)] --> Naver["authenticated Naver sync"]
        Upstream["SCRMapDocs / eudplib / eud-book / EUD Editor 3"] --> Public["pinned public-source sync"]
        Naver --> Scraper["Node.js + TypeScript<br/>tools/scraper"]
        Public --> Scraper
        Scraper --> Corpus["corpus JSONL<br/>ci/corpus/*.jsonl (committed)"]
    end
    Corpus -- "git commit + push" --> Repo[(eud-agent repo)]
    subgraph CI["GitHub Actions (no cookie, no ECA token)"]
        Repo --> Build["build_rag_index (Rust)<br/>--corpus ci/corpus"]
        Build --> Bin["rag-index.bin + .sha256 + manifest"]
        Bin --> Release[(GitHub Release rag-index-v*)]
    end
    Release -- "first-run bootstrap (feature 10)" --> App["eud-agent app (download + sha256)"]
```

## Scraper (Node.js + TypeScript, LOCAL)
- Location: `tools/scraper/` (its own `package.json` + `tsconfig.json`, separate from `panel/`).
  TypeScript ~5.9 (matches the panel convention). Run with `tsx`/`node`; not part of any runtime
  bundle and not invoked by CI.
- `npm run scrape`: authenticated Naver board/article API refresh. A Naver login **cookie** is
  supplied via env/file (NEVER committed); the scraper fails fast with guidance if the cookie is
  missing or rejected.
- `npm run sync-public`: no-secret shallow snapshot of SCRMapDocs, eudplib, eud-book, and EUD
  Editor 3. Every row records an immutable upstream commit and a commit-pinned source URL; project
  version, language, path, and scope are retained where applicable.
- Outputs UTF-8 JSONL matching `ci/build_rag_index.rs` `JsonlRow`: required `title`, `content`, and
  `source`; optional `id`, `url`, and `comments`. Public rows add provenance metadata ignored by
  the runtime parser.
- The fixed index inputs are `articles.jsonl`, `cafebook.jsonl`, `eud_book.jsonl`,
  `scrmapdocs_en.jsonl`, `eudplib_api.jsonl`, `eudplib_examples.jsonl`, and
  `eud_editor_schema.jsonl`.
- Naver requests are throttled and incremental; public snapshots are deterministically ordered and
  replaced atomically.

## Corpus (in-repo, committed, NOT LFS)
- `ci/corpus/*.jsonl` is the source of truth, replacing the ECA repo. Plain-text JSONL → normal git
  (diffs/compresses fine). NOT Git LFS (LFS is for the chromadb sqlite that we do not use).
- The legacy `ECA/chromadb_bge/chroma.sqlite3` is v1 and unused by v2 — out of scope, not imported.

## Embed (CI, unchanged format, ECA coupling removed)
- `ci/build_rag_index` reads the in-repo corpus (default path `ci/corpus`); the `--eca` flag is
  replaced/repurposed by a `--corpus <dir>` flag (default `ci/corpus`). It still produces
  `rag-index.bin` + `rag-index.bin.sha256` + `rag-index.manifest.json` (fastembed bge-m3 brute-force
  index — feature 12). No cookie, no ECA token required.
- `.github/workflows/build-rag-index.yml`: the "Checkout ECA corpus" step and
  `vars.ECA_REPO`/`secrets.ECA_TOKEN` are removed; the builder runs against the checked-out repo's
  `ci/corpus`. Triggers: `workflow_dispatch`, `rag-index-v*` tag push (existing), and optionally a
  push touching `ci/corpus/**`.

## Distribution
- The binary layout remains v2. The refreshed corpus is published as release generation
  `rag-index-v3`; `REQUIRED_RAG_INDEX_VERSION = "3"` makes healthy v2 installations fetch the v3
  manifest and replace their index through the existing sha256-verified atomic path.

## Edge cases
- Missing/expired cookie -> scraper exits non-zero with a clear "refresh Naver cookie" message; never
  writes a partial corpus file (write to `*.tmp` then atomic rename).
- Empty/short corpus -> the embed step should refuse to publish a near-empty index (sanity threshold).
- Re-scrape determinism: stable ordering (by board/post id) so commits are minimal content diffs.

## Implementation
- `tools/scraper/` — authenticated Naver API refresh plus commit-pinned public repository
  extraction; local only.
- `ci/corpus/*.jsonl` — seven fixed builder inputs plus `THIRD_PARTY_NOTICES.txt`.
- `ci/build_rag_index.rs` — fixed input allowlist, source-tier derivation, and
  `--corpus <dir>` (default `ci/corpus`).
- `.github/workflows/build-rag-index.yml` — embeds the checked-in corpus without ECA credentials.
- harness: `rules.md`, `architecture.md`, `tech-stack.md`, `features/12_rust-rag-fastembed.md`
  aligned to the in-repo corpus (Decision 15).
- external: `undici` HTTP/fetch client, `cheerio` HTML parsing, and local `git` for pinned shallow
  public snapshots.
