---
task_id: EUD-144-013d
completed_at: 2026-06-10T20:55:00
status: done
duration_minutes: 140
coding_retries: 2
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
  coder_session_id: null
  coder_tokens:
    input: 1004518
    output: 10484
    total: 1015002
  reviewer_tracked: false
---

## Summary
MERGED after a safety retry. The task adds an explicit, overridable embedding batch size
(`--batch` flag / `BATCH_SIZE` env / `DEFAULT_BATCH_SIZE`) and explicit ORT intra-op thread
configuration to `ci/build_rag_index.rs`. The first implementation defaulted the batch to 64;
differential verification showed the BGEM3Q int8 model's embeddings are **batch-size-dependent**
(batch 64 vs 16: per-row cosine median 0.9807, min 0.9645 on a 316-row subset; an independent
full-corpus comparison measured the same ~2% uniform shift across all 8445 rows), violating the
"no semantic change" criterion. A parallel session initially cancelled the task on that evidence
(commit ec0e396, default-64 variant). This session's retry restored `DEFAULT_BATCH_SIZE = 16`,
making the default output **byte-identical (sha256) to the untuned build** while keeping the
override knob and explicit thread config. User chose to merge this safe variant, superseding the
cancellation; task state corrected cancelled -> done.

## Changes
- `ci/build_rag_index.rs` — `--batch <n>` flag + `BATCH_SIZE` env (CLI > env > default 16, zero
  rejected), `embed(&texts, Some(batch_size))`, `with_intra_threads(available_parallelism)`,
  guard comment documenting the int8 batch-size-dependence constraint, unit test for batch
  resolution precedence.

## Verification
- `cargo test --manifest-path ci/Cargo.toml` — 2 passed (incl. verify-first artifact
  `resolve_batch_size_uses_cli_then_env_then_default`).
- `cargo fmt --manifest-path ci/Cargo.toml -- --check` — clean.
- `cargo build --release --manifest-path ci/Cargo.toml --bin build_rag_index` — succeeds.
- Differential test (316-row subset, 40 lines per corpus JSONL, warm model cache):
  - untuned (main) binary: sha256 `d13d70ea…`, 84.8 s
  - tuned binary, default (batch 16): sha256 `d13d70ea…` — **byte-identical**, 82.1 s
  - tuned binary, `--batch 64`: sha256 differs; cosine(old,new) median 0.9807, min 0.9645 —
    rejected as default; also ~23% slower (103.9 s)
  - tuned binary, `--batch 16` explicit: byte-identical — proves `with_intra_threads` alone has
    ZERO output effect (the drift cause is batch size only; fastembed already defaults
    intra_threads to `available_parallelism`, confirmed in fastembed source during review)
- Row count: subset 316 matches an independent chunking-replication script; full corpus expected
  8445 matches both the replication script and the parallel session's full tuned-build run.
  Per user policy (2026-06-10), full ci/corpus builds (~30-40 min) are NOT run for builder
  verification unless the embedding model/semantics change — subset differential suffices.

## Review
- codex review (temp clone, base d23a10b): "No actionable correctness issues" — no P1/P2/P3.
- An earlier round in the prior session fixed a P3 (defer BATCH_SIZE env parse until after the
  CLI loop). No blocking findings in either round.

## Notes
- A parallel Claude session raced this pipeline on the same repo: it committed the cancellation
  (ec0e396, with a report claiming worktree/branch cleanup that had not actually happened), then
  deleted the worker branch + worktree registration mid-pipeline. Commits were recovered from the
  object store (`git branch hv-worker/EUD-144-013d 05f15b6`). Single-session-per-repo is the safe
  topology.
- `codex review` refuses linked git worktrees ("Not inside a trusted directory") because the
  worktree `.git` is a file, not a directory; a trust entry in config.toml does not help. The
  review ran in a temporary local clone instead.
- The task body's premise "batching only affects throughput, not per-input embeddings" is
  empirically false for the int8 BGEM3Q model — only batch 16 reproduces the published v1 index.

## Incident

### What broke
- Completion criterion "no semantic change to embeddings" failed on the first implementation:
  defaulting batch to 64 shifted ALL embeddings uniformly (subset cosine median 0.9807,
  min 0.9645; full corpus mean 0.98, min 0.959), and was also ~23% slower than batch 16 locally.

### Why
- bge-m3 BGEM3Q is int8-quantized: dynamic quantization scales depend on batch composition, so
  the inference batch size systematically changes output vectors. Thread count is NOT a factor
  (isolated: batch 16 + explicit intra_threads is byte-identical to the untuned build).

### What fixed it
- Coding retry 2: restored `DEFAULT_BATCH_SIZE = 16` with a guard comment; the knob and thread
  config remain. Default output is byte-identical to the untuned build, so the published
  `rag-index-v1` embedding space (EUD-107 parity) is preserved.
