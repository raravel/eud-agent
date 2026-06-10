---
task_id: EUD-144-013d
completed_at: 2026-06-10T19:00:00
status: cancelled
duration_minutes: 55
coding_retries: 1
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: false
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
  coder_session_id: null
  coder_tokens:
    input: 637934
    output: 6644
    total: 644578
  reviewer_tracked: false
---

## Summary
CANCELLED by user decision after verification. The task tuned embedding batch size
(16 -> 64) and set ORT intra-op threads in `ci/build_rag_index.rs` to speed up the CI
embedding step. Implementation was correct and tests passed, but a direct before/after
index comparison showed the tuning **systematically changes every embedding** — so the
marginal CI speedup was not worth altering the index output. The complementary EUD-143
(ubuntu-latest + fastembed model cache, merged) already delivers the cost win.

## What was implemented (NOT merged)
- `resolve_batch_size(cli, env)` (cli > env > `DEFAULT_BATCH_SIZE=64`), `--batch <n>` flag +
  `BATCH_SIZE` env, threaded `batch_size` through `main` -> `embed_docs`.
- `docs.chunks(batch_size)` + `embed(&texts, Some(batch_size))`.
- `Bgem3InitOptions::with_intra_threads(available_parallelism())`.
- A unit test for batch resolution (verify-first red -> green), plus a P3 review fix
  (defer `BATCH_SIZE` env parse until after the CLI loop).

## Verification (the reason for cancellation)
Built a fresh index with the tuned builder (`--cache .fastembed_cache`, default batch 64) to
`rag-index-new.bin` and compared it byte/vector-wise to the v1 index `rag-index.bin`
(built with the untuned batch-16 builder):
- rows written: 8445 (matches); file size identical (52,751,390 B); ids/text/source: 0 mismatches.
- sha256 differs (a092ac78… vs b4609041…) — NOT byte-identical.
- Per-row cosine(old, new) distribution over all 8445 rows: 100% fall in 0.95–0.99;
  mean 0.9799, median 0.9803, p1 0.9700, min 0.9587; max abs element diff 0.042.

The shift is uniform across ALL rows (not a few edge cases) = a systematic numerical change,
well beyond floating-point noise. Cause: bge-m3 BGEM3Q (int8-quantized) is sensitive to the
inference batch size and ORT thread count. Completion criterion "no semantic change to
embeddings vs the untuned build" therefore FAILS.

## Decision
User chose to revert. Rationale: the speedup is marginal, the change alters every embedding
~2%, and EUD-143 already covers the cost/runner optimization. The worker branch is NOT
merged; the builder stays bit-reproducible (batch 16, no thread override), matching the
published v1 index.

## Notes
- Worktree and branch were cleaned up (cancellation by decision, not a failure to diagnose).
- Test artifact `rag-index-new.bin` removed; published `rag-index-v1` is unaffected and remains usable.
- Retrieval impact was not separately measured: the runtime embeds queries at batch 1, so an
  index built at batch 16 vs 64 is internally consistent either way — but reproducibility
  against v1 was the deciding factor.

## Incident

### What broke
- Completion criterion "no semantic change to embeddings" failed: the batch(16->64)+thread
  tuning shifted all 8445 embeddings (cosine old/new mean 0.98, min 0.959, max abs diff 0.042).

### Why
- bge-m3 BGEM3Q is int8-quantized; inference batch size and ORT intra-op thread count change
  the quantized accumulation path, producing a uniform ~2% rotation of every output vector.

### What fixed it
- Not a code fix — cancelled the task. Kept the builder bit-reproducible; relied on EUD-143
  (ubuntu + model cache) for the CI optimization instead.
