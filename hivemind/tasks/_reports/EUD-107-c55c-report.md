---
task_id: EUD-107-c55c
completed_at: 2026-06-08T15:17:00
duration_minutes: 35
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 8
  safety: 10
  clarity: 9
tokens:
  estimated: true
  input: 130000
  output: 42000
cost_usd: 5.10
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Embedding-parity gating spike for the RAG story. Confirmed the Rust **fastembed bge-m3 (int8,
`Bgem3Model::BGEM3Q`, dense 1024d, L2-normalized)** path embeds into the same space as the
Python **`sentence-transformers` BAAI/bge-m3** baseline: for a fixed 167-doc ECA-corpus subset
+ 10 Korean queries, top-5 set overlap is **9/10 queries ≥4/5 (mean 3.90/5)**. The retrieval
SET (what feeds the LLM context) matches the full-precision baseline — the gating question is
answered: **fastembed bge-m3 is usable for RAG.**

Spike FINDING: rank ORDER *within* the top-5 drifts (order-agreement 0–4/5) — the int8 model
recovers nearly the same set but reshuffles it. See "Finding" below.

## Changes
- `src-tauri/src/rag.rs` (+256) — `Embedder` over `fastembed::Bgem3Embedding` (BGEM3Q int8,
  dense 1024d, explicit L2-normalize since dense is NOT pre-normalized), `l2_normalize` /
  `cosine` / `top_k` helpers, typed `RagError`, and `#[cfg(test)] mod parity` with the
  `#[ignore]` parity test. Minimal by design — warmup/index-load/threading are a later task.
- `src-tauri/src/lib.rs` (+1) — `pub mod rag;`.
- `ci/gen_rag_parity_fixture.py` + `src-tauri/tests/fixtures/rag_parity.json` — Python baseline
  half (committed earlier as `fe1c38f`); ECA read-only.
- `hivemind/docs/features/12_rust-rag-fastembed.md` — recorded the chosen variant + L2 norm +
  parity result (completion criterion #2).
- `.gitignore` — `.fastembed_cache/` (the ~560MB int8 model the test writes CWD-relative when
  no cache dir is set; never committed).

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- `cargo test -p eud-agent rag::parity -- --ignored` → **passed** (66.91s; model cached).
  Reproduced the worker's per-query numbers exactly: 9/10 queries ≥4/5, mean 3.90/5.
- `cargo clippy -p eud-agent --all-targets -- -D warnings` → clean. `cargo fmt --check` → clean.
- Scope: only `src-tauri/src/rag.rs` + `src-tauri/src/lib.rs` in the worker diff (fixture +
  generator pre-committed; feature-doc + .gitignore are orchestrator harness/hygiene).
  merge-base = `main`, not stale.

## Review
Reviewer (opus-4-7): **no blocking findings**; rubric correctness 9 / spec_compliance 8 /
safety 10 / clarity 9. Verified the parity number is computed correctly — shared contiguous
index space (corpus id == position), required L2-normalization present (against fastembed 5.16
source: dense is raw/un-normalized), correct BGEM3Q/dense API, panic-free top_k with
deterministic tie-break. Advisory: the "matching order" half of the criterion is relaxed to
reported-not-asserted (defensible for a measurement spike; recorded here). Minor robustness
follow-ups (assert `fixture.normalized`/dim) noted, not applied.

## Finding (the spike's actual output — needs a downstream decision)
- **Set parity: GOOD.** int8 fastembed recovers ≥4/5 of the full-precision top-5 for 9/10
  queries. The top-k retrieval set is reliable.
- **Rank order: DRIFTS.** Within the top-5, order agreement is 0–4/5 (q2/q6/q9 = same items,
  fully reshuffled). Expected int8-quantization effect.
- **Decision**: int8 (BGEM3Q) is acceptable for RAG retrieval — the top-k SET is the contract
  and it feeds the LLM unordered. If exact rank order ever matters downstream, re-evaluate full
  precision. The CI index builder MUST use the SAME BGEM3Q int8 + L2 norm so the at-rest index
  and the runtime query share the space (recorded in feature 12).

## Harness Sync
- `src-tauri/src/rag.rs` already listed in feature 12 `## Implementation` — binding no-op.
- No manifest change (fastembed/serde/serde_json already deps). Contract-drift guard: additive
  only; feature-12 edit RECORDS the spike result (criterion #2 mandate), not a contract change.

## Notes
- Worker worktree was stale-based again (branched from POC `23bc6f4`); the upfront prompt
  instruction had it `git reset --hard main` + provision the gitignored `panel/dist` (needed by
  `tauri::generate_context!()`) within Step A — no extra round-trip (coding_retries 0).
- Follow-ups for the RAG warmup/index task: point the embedder cache at `DataDirs::models_dir()`
  (the test used fastembed's default CWD cache); add `assert!(fixture.normalized)` robustness.
