---
task_id: EUD-108-6439
completed_at: 2026-06-10T00:45:00
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: true
  input: 0
  output: 0
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.5
  reviewer: gpt-5.5
codex_usage:
  coder_session_id: 019eacbf-3eed-7222-93e2-eaf1db0ff809
  coder_tokens:
    input: 4379663
    output: 29425
    total: 4409088
  reviewer_tracked: false
---

## Summary
Standalone CI builder `ci/build_rag_index.rs` that re-embeds the ECA corpus with fastembed bge-m3
**BGEM3Q** (int8, dense 1024d, L2-normalized — the SAME variant the runtime query path uses, so the
at-rest index and the runtime share the embedding space) and writes `rag-index.bin` in the exact
`ERAG`/v1 layout + a sha256 sidecar. std-only parsing — NO `rusqlite`/sqlite; the ECA repo and its
chromadb are read-only/never imported (only the JSONL corpus is read).

## Changes (scope ci/**)
- `ci/Cargo.toml` — standalone crate (own `[workspace]`, detached from the app workspace); deps
  `fastembed=5.15`, `serde`, `serde_json`, `sha2=0.10`, `anyhow`. NO `rusqlite`.
- `ci/build_rag_index.rs` — reads `articles.jsonl`+`eud_book.jsonl`+`cafebook.jsonl`; per row builds
  `"제목: {title}\n\n{content}"` (+ `\n\n[댓글]\n{comments}`), source `[{title}]({url})`; CHUNKS long
  docs into 2000-char windows (200 overlap, on char boundaries) so each hit is bounded and each chunk
  is fully embedded; embeds with BGEM3Q + L2-normalize; writes `ERAG`/v1 (`magic|version=1|count|
  rows: id u64 | 1024×f32 LE | text_len u32+text | source_len u32+source`); writes `<out>.sha256`.
  Stable per-chunk id = FNV-1a64 of `"{stable_key}#{chunk_index}"`.
- `ci/README.md` — reproducible run (command, inputs, outputs, one-time ~570MB model download).

## Verification (orchestrator-run)
- `cargo build --release --manifest-path ci/Cargo.toml` — compiles + links (the worker's sandbox
  could only type-check; the real ort link succeeds here).
- Real run vs the ECA corpus: **8445 rows** written, sha256 emitted, `rag-index.bin` ~52.7MB. (Model
  downloaded once to the gitignored `.fastembed_cache/`.)
- Byte-compat round-trip: a temporary harness test loaded the produced `.bin` via the runtime
  `eud_agent_lib::rag::load_index` — **8445 entries**, every vector 1024-d, **max entry text = 2000
  chars** (chunk budget holds). Temp test reverted. [criteria 1,2]
- `cargo clippy --manifest-path ci/Cargo.toml --all-targets -- -D warnings` clean; `cargo fmt
  --manifest-path ci/Cargo.toml -- --check` clean.
- No `rusqlite`/sqlite dependency; only the 3 JSONL files are read. [criterion 3]
- Reproducible run documented in ci/README.md. [criterion 4]

## Review
codex review (`--base main`) returned one finding:
- [P2] long corpus rows (e.g. eud_book.jsonl line 1 ~106k chars) stored as one entry would be
  injected verbatim into the prompt (engine.rs `reference_context_section` does NOT cap `hit.text`,
  and `Rag::search` returns up to 10 hits) and the embedding only covers the truncated prefix. REAL;
  fixed by adding the 2000-char chunking (one review round).

## Harness Sync
- `ci/build_rag_index.rs` is already documented in features/12_rust-rag-fastembed.md `## CI index
  builder` / `## Implementation` — no new binding. `ci/Cargo.toml` is a standalone builder manifest
  whose deps (fastembed/sha2/serde) already exist in the app tech-stack — no tech-stack change.

## Notes / Deviation
- Row count is **8445**, not the spec's "~4,974 (~20MB)". The "~4974" was the OLD chromadb_bge figure
  (built by ECA `rebuild_bge.py` from a smaller `articles.jsonl` snapshot + sqlite "manuals", with the
  full doc stored but the embedding truncated). The v2 builder cannot use that path (it would need the
  forbidden sqlite). It uses the current 3-JSONL corpus (articles 4920 + eud_book 629 + cafebook 83 =
  5632 non-empty rows) and then CHUNKS long rows → 8445 entries / ~52.7MB. This is a deliberate,
  documented departure that is MORE correct (no sqlite, every chunk fully embedded, bounded hits).
  Suggest the user re-ground feature 12's "4,974 rows (~20MB)" line via /hv:plan.
- Model: profile `mixed` -> `gpt-5.2-codex` is rejected on this ChatGPT-account codex; used `gpt-5.5`.

## Incident

### What broke
- Code review [P2]: huge ECA rows (≈106k / ≈57k chars) would be embedded as single entries and
  injected verbatim into the Codex prompt, bloating/overflowing it; embedding only saw the prefix.

### Why
- The first builder pass did one-entry-per-row with no length bound; the runtime injects `hit.text`
  uncapped and bge-m3 truncates long inputs at the tokenizer limit.

### What fixed it
- Added char-boundary chunking (CHUNK_CHARS=2000, CHUNK_OVERLAP=200) with per-chunk stable ids and
  `(part n/total)` source markers. Verified: max stored entry = 2000 chars, 8445 entries round-trip
  through `rag::load_index`. Fixed on the single review round (codex exec resume of the coder session).
