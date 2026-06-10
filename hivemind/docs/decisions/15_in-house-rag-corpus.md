# Decision 15: In-house RAG corpus pipeline (Naver Cafe scraper + in-repo corpus), ECA decoupled

- Date: 2026-06-10
- Status: Accepted
- Supersedes (partial): [[decisions/10_rag-bruteforce-fastembed]] — the "corpus stays in the ECA repo / RAG index is a CI artifact built from ECA" aspect only. The brute-force fastembed `.bin` index format and its GitHub-Release distribution are UNCHANGED.

## Context
The RAG corpus (`articles.jsonl`, `eud_book.jsonl`, `cafebook.jsonl`) lived in the separate
read-only **ECA** repo. The CI index builder (EUD-118 `build-rag-index.yml`) checked ECA out via
`vars.ECA_REPO` + `secrets.ECA_TOKEN` to embed it. The corpus and the original embeddings were
the user's own work; keeping them in ECA only coupled eud-agent to a private repo + token for no
necessary reason. The true first step — scraping Naver Cafe posts — needs a Naver **login cookie**,
which cannot live cleanly in CI (secret + fragile login automation + ToS). So eud-agent should own
the whole pipeline and the scrape step must run locally.

A key clarification drove the distribution choice: the old `rules.md`/Decision 10 prohibition
"NEVER LFS the rag db — chromadb mutates tracked sqlite on open (LFS churn)" is **chromadb-specific**
(the legacy v1 `ECA/chromadb_bge/chroma.sqlite3`). The v2 index is a static, write-once,
read-only `rag-index.bin` (`src-tauri/src/rag.rs`) the app only reads — it does NOT churn. LFS would
therefore have been technically safe, but the deciding factor is git history bloat (LFS retains every
~50MB revision), so the Release asset is kept.

## Considered
- Output distribution: GitHub Release asset vs Git LFS (in-repo / bundled) — Pros(LFS): version-locked, could ship in bundle (no first-run RAG download). Cons(LFS): ~50MB retained per index revision in git/LFS history. Recommendation: Release ★★★ — keeps history clean; bootstrap (feature 10) unchanged.
- Scraper language: Node.js+TypeScript vs Python vs Rust — Pros(Node/TS): general, lighter than Rust for scraping, rich HTTP/HTML ecosystem. Cons: adds a Node toolchain tool (Python was just removed in EUD-121; Node/TS avoids reintroducing Python). Recommendation: Node/TS ★★★.
- Embed build location: keep CI vs fully local — Pros(CI): reproducible, no 570MB model re-download per run, auto re-embed on corpus change. Cons: none once the corpus is in-repo (no cookie/ECA needed for embed-only). Recommendation: CI ★★★.

## Chosen
In-house pipeline: a **local-only Node.js+TypeScript Naver-Cafe scraper** (cookie-based) produces the
corpus JSONL → the corpus JSONL is **committed in-repo** (e.g. `ci/corpus/*.jsonl`, plain git, not
LFS) as the source of truth → the **CI** `build-rag-index.yml` embeds the in-repo corpus (ECA
checkout + `ECA_REPO`/`ECA_TOKEN` removed) → publishes `rag-index.bin` + sha256 + manifest to a
**GitHub Release** (distribution unchanged; app first-run bootstrap still downloads from the Release).

## Rationale
The scrape needs a Naver cookie → must be local; once the corpus is committed, embedding needs
neither cookie nor ECA token, so CI is kept for reproducible re-embed + Release publish. Node/TS is
light and general and avoids reintroducing the Python stack just removed. Release distribution keeps
git history clean and leaves the consumer/bootstrap path untouched.

## Impact (harness files to align)
- `rules.md` — drop "NEVER modify the ECA repo / corpus is read-only input in ECA / NEVER import chromadb_bge"; replace with "corpus JSONL lives in-repo; the chromadb-churn caveat is scoped to chromadb only; the static `.bin` is still a Release artifact, not committed".
- `architecture.md` — the "RAG corpus source stays in the ECA repo" line + the component-diagram corpus/HF/GHR notes.
- `tech-stack.md` — add the Node/TS scraper tool + its deps; note ci/build_rag_index reads in-repo corpus.
- `features/12_rust-rag-fastembed.md` — corpus input now in-repo.
- `features/16_rag-corpus-pipeline.md` — NEW, the end-to-end pipeline spec (this decision's home).
- `.github/workflows/build-rag-index.yml` — remove ECA checkout + secrets/vars.
- `ci/build_rag_index.rs` — default corpus path to the in-repo `ci/corpus`, drop the ECA default.
- Note: committing scraped Naver-Cafe content carries a user-acknowledged ToS/copyright risk.
