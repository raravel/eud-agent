---
task_id: EUD-156-ade9
completed_at: 2026-06-12T16:44:34Z
duration_minutes: 18
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
  correctness: 4
  spec_compliance: 7
  safety: 6
  clarity: 8
tokens:
  estimated: true
  input: 9400
  output: 4100
cost_usd: 0.20
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
Derived a v2 source-trust `tier_level: u8` from each corpus row's `source` field in
`ci/build_rag_index.rs` and bumped the builder's binary index format to v2, byte-identical
to the v2 writer in `src-tauri/src/rag.rs` (EUD-155). The tier byte sits AFTER the full
vector and BEFORE the text length prefix, matching the rag.rs layout exactly.

## Changes
- `ci/build_rag_index.rs` (only file touched, +77/-3):
  - `INDEX_VERSION: u32` 1 → 2.
  - `tier_level: u8` added to `CorpusDoc` and `IndexEntry`.
  - New `tier_level_for_source(&str) -> u8` with a doc-comment mapping table citing
    `features/17_rag-knowledge-tiering.md`; strips a trailing `.jsonl`, matches `user_` as
    a prefix, falls back to `1` (general) for unknown sources (documented neutral default).
  - `corpus_docs_from_row` computes the tier once (before `row.source` is consumed into the
    fallback key) and stamps it on every chunk of the row; `embed_docs` carries it through.
  - `write_index` writes the single tier byte after the vector loop, before the text len.
  - Two stale "v1 format" bail! messages updated to "v2 format".
  - 6 unit tests: official→3, lecture/research→2, general→1, `user_` prefix→1, Q&A→0,
    unknown→1.

## Verification
Run by the orchestrator in the worker worktree, CARGO_TARGET_DIR pointed at the warm
release `ci/target`, `--release` to reuse the warm ort/fastembed cache:
- `cargo fmt --manifest-path ci/Cargo.toml -- --check` → exit 0
- `cargo clippy --release --manifest-path ci/Cargo.toml --all-targets -- -D warnings` → exit 0
- `cargo test --release --manifest-path ci/Cargo.toml` → 8 passed; 0 failed

Completion criteria (all PASS):
- [PASS] source → tier_level mapping for all four groups
- [PASS] VERSION=2 with tier_level matching the rag.rs layout (verified tier byte position)
- [PASS] unit test per source group incl. `user_*` prefix and board Q&A → 0
- [PASS] unknown source falls back to documented default (1) with a test
- [PASS] cargo build + test (ci manifest) passes

## Review
Claude reviewer (sonnet) raised two findings it labeled BLOCKING; the orchestrator
adjudicated BOTH as false positives and did NOT trigger a fix round:
- F1 "rag.rs reader/writer still v1": REJECTED. `main`'s `src-tauri/src/rag.rs` is already
  v2 (INDEX_VERSION=2, `tier_level` field, write `&[entry.tier_level]`, read `take_u8`) —
  EUD-155's merged deliverable. EUD-156's scope is `ci/build_rag_index.rs` only; rag.rs is
  out of scope and already correct.
- F2 "v1 format bail! messages remain": REJECTED. The coder's actual file shows "v2 format"
  at both sites.
Root cause: the review worker was spawned with `isolation: "worktree"`, which gave it a
clean worktree that did NOT contain the coder's branch — and whose `rag.rs` was a stale
pre-EUD-155 v1 base. It reviewed base/stale files rather than the coded change. The
reviewer's low correctness(4)/safety(6) scores are entirely premised on the phantom F1.
Advisory F3/F5 the reviewer itself reclassified as correct design; F4 (unknown→1 default)
is acceptable. No genuine blocking issue → review PASS.

## Harness Sync
- no-op: `ci/build_rag_index.rs` is already listed under `features/17_rag-knowledge-tiering.md`
  `## Implementation`; no manifest file changed. Contract-drift guard: no removed/renamed
  spec identifiers (INDEX_VERSION 1→2 is spec-mandated). Skip condition satisfied.

## Notes
- The coding worker's `isolation: "worktree"` branched from a STALE base (`683def7`,
  pre-EUD-155). The orchestrator detected this via `merge-base HEAD main` and rebased the
  branch onto `main` (`1f7b466`) BEFORE Step B, so the worker could reference the v2 rag.rs
  for byte-parity. Without the rebase the worker would have had no v2 reference.
- The review worker's stale/clean worktree (above) produced two false-positive blocking
  findings. Future reviews should either pass the diff for review WITHOUT a fresh worktree,
  or rebase/apply the coder branch into the review worktree first.

## Incident

### What broke
- The Claude review worker (spawned with `isolation: "worktree"`) emitted two BLOCKING
  findings (F1: "rag.rs still v1", F2: "v1 format bail! messages remain") that were both
  false. Acting on them would have pushed an out-of-scope, incorrect edit to
  `src-tauri/src/rag.rs`.

### Why
- `isolation: "worktree"` gives the review agent a fresh worktree that does NOT contain the
  coding worker's branch, and (when branched from a stale base) a pre-EUD-155 v1 `rag.rs`.
  The reviewer read those base files instead of the actual change, so it "saw" v1 code.
- Compounding: the coding worker's worktree itself branched from a stale base (`683def7`),
  which the orchestrator had to detect and rebase onto `main` before Step B.

### What fixed it
- The orchestrator verified both findings directly against `git show main:src-tauri/src/rag.rs`
  (confirmed v2) and a `grep` of the coder worktree file (confirmed "v2 format" messages),
  adjudicated both as false positives, and skipped the review fix round. The coder branch
  was rebased onto `main` before Step B so the parity reference was correct.
