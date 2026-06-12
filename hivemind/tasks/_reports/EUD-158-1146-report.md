---
task_id: EUD-158-1146
completed_at: 2026-06-12T17:27:27Z
duration_minutes: 22
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: claude
  reviewer: claude
review_scores:
  correctness: 9
  spec_compliance: 10
  safety: 8
  clarity: 9
tokens:
  estimated: true
  input: 13800
  output: 6400
cost_usd: 0.38
profile: balanced
models:
  executor: opus
  reviewer: claude-sonnet-4-6
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: null
    output: null
    total: null
  reviewer_tracked: false
---

## Summary
Added a CI migration binary `ci/migrate_rag_index.rs` that upgrades the published v1
`rag-index.bin` to v2 (per-chunk `tier_level`) WITHOUT re-embedding: it reads v1
vectors+text+source, re-parses `ci/corpus` to derive each chunk's `id = fnv1a64(chunk_key)`
and `tier_level`, joins on id (all-or-nothing), copies vectors byte-for-byte, and writes the
v2 layout byte-identical to `build_rag_index.rs` / `rag.rs`.

## Changes
- `ci/migrate_rag_index.rs` (new, 811 lines): pure derivation fns duplicated VERBATIM from
  `build_rag_index.rs` (`fnv1a64`, `chunk_text` 2000/200, `tier_level_for_source`, `JsonlRow`,
  the corpus read over the FIXED `INPUT_FILES` list, and the `chunk_key` logic) — yielding
  only `(id, tier_level)` per chunk; an own bounds-checked v1 reader (rag.rs rejects v1, v1
  has no tier byte); `build_v2_entries` all-or-nothing join (v1-orphan AND corpus-orphan both
  hard-error); a v2 writer byte-identical to build_rag_index/rag.rs; a `#[cfg(test)]`
  `read_v2_index`; a `--in/--corpus/--out` CLI. 3 tests: byte-for-byte vector preservation +
  tier stamping, and both orphan directions.
- `ci/Cargo.toml` (+4): the `[[bin]] name = "migrate_rag_index"` target only.

## Verification
Run by the orchestrator in the worker worktree, CARGO_TARGET_DIR = warm `ci/target`, --release:
- `cargo fmt --manifest-path ci/Cargo.toml -- --check` → exit 0 (both ci files clean).
- `cargo clippy --release --manifest-path ci/Cargo.toml --all-targets -- -D warnings` → exit 0.
- `cargo test --release --manifest-path ci/Cargo.toml` → build_rag_index 8 passed +
  migrate_rag_index 3 passed; 0 failed.

I also diffed the duplicated derivation fns against `build_rag_index.rs` directly (fnv1a64
seed/prime, chunk_text stride/break, tier match arms + order, chunk_key fallback order,
INPUT_FILES) — byte-for-byte identical, so the id-join is sound.

Completion criteria (all PASS):
- [PASS] ci/migrate_rag_index.rs + [[bin]] entry in ci/Cargo.toml
- [PASS] reads v1 + corpus, joins on fnv1a64(chunk_key), writes v2 with tier_level
- [PASS] vector-preservation test: every v2 vector byte-identical to its v1 source
- [PASS] unmatched id (either direction) is a hard error (both tested)
- [PASS] cargo build + test (ci manifest) passes

## Review
Claude reviewer (sonnet) read the actual change from the coder branch via `git show <branch>:`
and `git diff main..<branch>` (instructed to avoid the EUD-156 stale-worktree trap) — no false
positives. The highest-risk check (Finding 1, derivation parity vs build_rag_index.rs) was
confirmed byte-for-byte identical. All findings advisory; no fix round:
- `Vec::with_capacity(count)` on an untrusted file count is uncapped (rag.rs caps with
  INDEX_CAP_HINT) — advisory; this is a CI tool over a ~5k-row trusted index, low real risk.
- The migration does NOT emit the `.sha256` sidecar that build_rag_index writes — advisory;
  sidecar/manifest republish belongs to feature 17's "Bootstrap + CI republish" track
  (a separate task), not this migration binary. See Notes.
- Corpus duplicate-id silent overwrite in the tier map (unlikely — chunk keys are unique) and
  orphan detection reporting only the first leftover — both advisory/cosmetic.
Rubric: correctness 9, spec_compliance 10, safety 8, clarity 9 — all at/above blocking
thresholds. Review PASS.

## Harness Sync
- idempotent: `ci/migrate_rag_index.rs` is already documented under
  `features/17_rag-knowledge-tiering.md` `## Implementation` (line 127, "ci/ migration binary
  (e.g. migrate_rag_index.rs)"). The `ci/Cargo.toml` change adds a `[[bin]]` target only — NO
  new dependency — so no tech-stack dep binding applies. Contract-drift guard: no removed or
  renamed spec identifier (the derivation fns are DUPLICATED, not moved). Nothing to append.

## Notes
- Marking this task done did not auto-complete its parent EUD-152-0709 (other children of
  EUD-152 remain).
- The coding worker proactively detected its `isolation: "worktree"` stale base (`683def7`)
  and rebased onto `main` (`cf606c7`) before Step A.
- The review worker was given the coder branch name + told to read via `git show <branch>:`
  rather than its own worktree — avoiding the EUD-156 false-positive class.
- Cross-task dependency to track: the `.sha256` sidecar + `rag-index.manifest.json` version
  bump for the migrated v2 asset is feature 17's CI-republish task (build-rag-index.yml /
  bootstrap manifest), NOT this migration binary. The migrated bin must be sha256-sidecar'd by
  whoever republishes it.
- The id/tier derivation is DUPLICATED across `build_rag_index.rs` and `migrate_rag_index.rs`
  (the ci crate has two bins; a bin cannot import another). The verbatim copy is guarded by the
  all-or-nothing join + the byte-for-byte vector-preservation test, but the two copies must
  stay in sync if the corpus/chunking ever changes.
