---
task_id: EUD-109-b35b
completed_at: 2026-06-08T07:33:03Z
duration_minutes: 18
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: claude
  reviewer: claude
review_scores:
  correctness: 9
  spec_compliance: 9
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 236000
  output: 59000
cost_usd: 7.96
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: null
    output: null
    total: null
  reviewer_tracked: false
---

## Summary
Implemented the `src-tauri/src/rag.rs` query path on top of the EUD-107 embedding surface:
a persisted at-rest index loaded fully into memory, a lazily-initialized embedder with a
background warmup that never gates app readiness (emits `rag_warmup`), and a brute-force
cosine `search` returning top-k `{text, source, score}`. The whole change is confined to
`rag.rs` (no new dependencies); the EUD-107 `Embedder`/`l2_normalize`/`cosine`/`top_k`/
`mod parity` surface is untouched.

## Changes
- `src-tauri/src/rag.rs` (only file): added
  - `RagError::Warming` (search before/while warming) and `RagError::Index(String)` (bad
    `.bin`).
  - `MAX_TOP_K = 10` and `SEARCH_DOCS_GUIDANCE` (v1 Korean-corpus guidance: query in
    Korean, keep eps/API identifiers verbatim, k clamped to 10).
  - `IndexEntry` / `Hit` structs.
  - `write_index` / `load_index` for a self-contained little-endian `.bin` format
    (magic `ERAG`, version `1`, count, then per record `id u64` + `EMBED_DIM*f32` +
    len-prefixed utf8 text/source). A bounded byte cursor maps every short read / bad
    magic / bad version / non-`EMBED_DIM` vector / non-utf8 to `RagError::Index` — never
    panics. `load_index` clamps the `with_capacity` hint (`INDEX_CAP_HINT = 65_536`) so an
    untrusted header `count` cannot trigger a huge speculative allocation.
  - `Rag` (in-memory index + `Mutex<Option<Embedder>>` + cache dir): `new` /
    `from_index_file` never load the model; `is_ready`; `warmup` (idempotent, emits
    `rag_warmup` 0→100 via `bootstrap::ProgressEmitter`); pure `rank` (cosine desc,
    tie-break by lower id, `k` clamped to `MAX_TOP_K`, empty → empty); `search`
    (`try_lock` → `Warming` if warmup is in flight or not yet done, never blocks; embeds
    then ranks).
- Tests added in `#[cfg(test)] mod query` (8 tests): `bin_roundtrip`,
  `load_index_rejects_truncated`, `rank_orders_by_cosine`, `rank_clamps_k`,
  `empty_index_returns_empty`, `search_before_warmup_is_warming`,
  `search_during_warmup_does_not_block`, `search_docs_guidance_mentions_korean`.

## Verification
Run by the orchestrator in the worker worktree with the shared target cache
(`CARGO_TARGET_DIR=...\.cargo-shared-target`):
- `cargo test -p eud-agent rag` → ok: 8 `rag::query` tests passed; `rag::parity` ignored
  (downloads ~570MB model — not run).
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → clean (exit 0).
- `cargo clippy -p eud-agent --all-targets -- -D warnings` → clean (exit 0).

Completion criteria:
- [PASS] `search(query, k)` returns ranked hits; cosine matches parity expectations
  (`rank` uses `cosine`, desc order, deterministic tie-break; the EUD-107 `rag::parity`
  test pins fastembed-vs-Python top-5 set overlap and stays `#[ignore]`d).
- [PASS] Warmup runs in the background; UI usable before model load completes
  (construction never loads the model; `is_ready` false until warmup; `search` returns
  `Warming` rather than blocking — proven by `search_before_warmup_is_warming` and
  `search_during_warmup_does_not_block`).
- [PASS] `cargo test rag` passes.

## Review
4-axis rubric (claude reviewer): correctness 9, spec_compliance 9, safety 9, clarity 10 —
no blocking scores, no blocking findings. Two advisory findings were fixed in one review
round (both within `rag.rs`):
1. `search` could block for the whole warmup window because `warmup` holds the embedder
   lock across the ~570MB model load while `search` used a blocking `lock()`. Switched
   `search` to `try_lock()` (`WouldBlock` → `RagError::Warming`), so it never gates on the
   model — satisfying the method contract and rules.md "RAG model loading must NEVER gate
   app readiness". Added `search_during_warmup_does_not_block` to pin it.
2. `load_index` passed the untrusted header `count` straight to `Vec::with_capacity`;
   clamped the allocation hint to `INDEX_CAP_HINT` (the read loop still consumes exactly
   `count` records, so a wrong count still surfaces as a truncation error).

The `.bin`-vs-sqlite index format is an intentional, user-approved divergence from feature
12's prose (no new `rusqlite` dependency; matches `bootstrap::RAG_INDEX_FILENAME =
"rag-index.bin"`; the index is loaded fully into RAM either way, so sqlite's query features
are unused). The index is loaded fully into memory with background warmup + model resident
(approach A, user-confirmed).

## Notes
- Harness sync (step 11.5): no-op — `src-tauri/src/rag.rs` is already listed under
  `features/12_rust-rag-fastembed.md` `## Implementation`, and no manifest changed. No
  binding appends, no contract drift (the diff is purely additive; no spec-promised
  identifier was removed or re-signatured).
- Follow-up (out of scope, for the CI builder task): `ci/build_rag_index.*` must emit this
  exact `ERAG`/v1 `.bin` layout so the at-rest index and the runtime query share the
  embedding space. The `index version mismatch → bootstrap re-download` policy (feature 10)
  is left to the integration/bootstrap wiring; `load_index` already rejects a wrong version
  as a typed `RagError::Index`.
- Spec doc drift to reconcile later: `features/12_rust-rag-fastembed.md` still describes a
  sqlite/rusqlite index format and lists `rusqlite` as an external dependency; the approved
  implementation uses the `.bin` format with no `rusqlite`. Update the feature doc via
  `/hv:plan` so the spec matches the shipped format.
