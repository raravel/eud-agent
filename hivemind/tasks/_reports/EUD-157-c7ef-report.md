---
task_id: EUD-157-c7ef
completed_at: 2026-06-12T17:01:56Z
duration_minutes: 14
coding_retries: 0
verify_retries: 1
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
  safety: 10
  clarity: 8
tokens:
  estimated: true
  input: 11200
  output: 5200
cost_usd: 0.30
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
Made `Rag::rank()` tier-aware in `src-tauri/src/rag.rs`:
`score = cosine(query, entry) * TIER_WEIGHT[entry.tier_level]`, with `TIER_WEIGHT` a
4-element code constant in a narrow band near 1.0 (`[1.00, 1.05, 1.10, 1.15]`) so source
tier nudges but never dominates the cosine signal. Tie-break (lower id) and the `MAX_TOP_K`
clamp are preserved. The weight is a code constant (retunable without rebuilding the index,
Decision 18); only the tier level is stored.

## Changes
- `src-tauri/src/rag.rs` (only file touched, +124/-9):
  - `const TIER_WEIGHT: [f32; 4] = [1.00, 1.05, 1.10, 1.15];` near `MAX_TOP_K`, doc-comment
    citing features/17 + Decision 18.
  - `rank()` multiplies cosine by the tier weight, indexed DEFENSIVELY:
    `TIER_WEIGHT.get(e.tier_level as usize).copied().unwrap_or(1.0)` — a corrupt
    `tier_level >= 4` falls back to weight 1.0 instead of panicking (rules.md no-panic).
  - `Hit.score` doc updated: it now carries the weighted score, not raw cosine.
  - Existing `rank_orders_by_cosine` adjusted (expected_top × TIER_WEIGHT[2]; order
    unchanged, orthogonal ~0 assertion preserved).
  - New `rank_is_tier_weighted` test: two documented cases — (1) near-tie cosine, higher
    tier ranks first (with a sanity assert that raw cosine favors the LOW-tier entry, so any
    reorder is attributable to the weight); (2) large cosine gap, low-tier high-cosine entry
    still ranks first (the band near 1.0 cannot flip it).

## Verification
Run by the orchestrator in the worker worktree, CARGO_TARGET_DIR = warm shared target
(`.cargo-shared-target`):
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` → 0 violations in rag.rs
  (8 pre-existing violations in bridge_install/engine/journal/tool_exec remain — they are on
  `main`, out of this task's scope; see Incident).
- `cargo clippy -p eud-agent --lib -- -D warnings` → exit 0.
- `cargo test -p eud-agent rag::` → 11 passed, 0 failed, 1 ignored (parity).

Completion criteria (all PASS):
- [PASS] TIER_WEIGHT 4-element constant near 1.0
- [PASS] rank() multiplies cosine by TIER_WEIGHT[tier_level]
- [PASS] test pins the weights + asserts tier influences order without overpowering cosine
- [PASS] tie-break lower-id and MAX_TOP_K clamp preserved (unchanged context lines)
- [PASS] cargo test passes

## Review
Claude reviewer (sonnet) read the actual change from the coder branch via
`git show <branch>:...` / `git diff main..<branch>` (explicitly instructed to avoid the
EUD-156 stale-worktree false-positive trap) — no false positives this time. All findings
advisory; no fix round triggered:
- Formula + defensive index correct.
- The new test proves both documented cases; reviewer noted it does not pin the exact
  cosine-gap boundary the band can flip (~13% for the top tier). That headroom is an inherent
  property of the spec-chosen weight band, not a defect — advisory only.
- Hit.score downstream: the reviewer found the two consumers (`tool_exec.rs` serializes
  `score` to the LLM as informational only; `engine.rs render_reference_hit` ignores it), so
  a weighted score up to 1.15 (above the raw-cosine ≤1.0 range) harms no consumer — the key
  safety check passed.
- tie-break / MAX_TOP_K preserved.
Rubric: correctness 9, spec_compliance 10, safety 10, clarity 8 — all above blocking
thresholds. Review PASS.

## Harness Sync
- no-op: `src-tauri/src/rag.rs` is already listed under
  `features/17_rag-knowledge-tiering.md` `## Implementation` ("TIER_WEIGHT, weighted rank()");
  no manifest changed. Contract-drift guard: rank() signature unchanged, TIER_WEIGHT is new
  (no removed/renamed spec identifier). Skip condition satisfied.

## Notes
- Marking this task done auto-completed the parent story EUD-151-e190 (all children done:
  EUD-154/155/156/157).
- The coding worker proactively detected its `isolation: "worktree"` had branched from a
  stale base (`683def7`, pre-EUD-155) and rebased onto `main` (`8e07357`) itself before
  Step A — without it the `tier_level` fixtures would not have compiled.
- The review worker was given the coder branch name and told to read the change via
  `git show <branch>:` rather than its own (stale) worktree files — this avoided the
  false-positive blocking findings seen in EUD-156.
- An unrelated `M src-tauri/Cargo.toml` was present in the working tree from session start;
  it was deliberately NOT staged into this task's commit.

## Incident

### What broke
- The verify.md `lint` stage (`cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`)
  failed. Two causes, only one in scope:
  1. The coder introduced TWO rustfmt violations in `rag.rs` (an over-long `.get().copied()
     .unwrap_or()` chain and a `Rag::new(vec![...], None)` call) — its Step B verify ran only
     `cargo test`, not `cargo fmt`, so they slipped through.
  2. `cargo fmt --check` ALSO reports 8 PRE-EXISTING violations on `main` in
     `bridge_install.rs:25`, `engine.rs:1216/1374/1409/1423`, `journal.rs:1243`,
     `tool_exec.rs:466/1119` — confirmed by running fmt --check on the clean `main` working
     tree. The repo's lint stage is already red independent of this task.

### Why
- (1) The coding worker's self-verify did not include the lint stage from verify.md; only
  the test stage was run.
- (2) Prior task commits landed without `cargo fmt`, accumulating fmt debt; the verify.md
  lint stage is not being enforced as a merge gate.

### What fixed it
- The orchestrator confirmed (via fmt --check on `main`) that the 8 other-file violations are
  pre-existing and OUT OF SCOPE (scope = rag.rs only), then sent the worker a targeted
  fix-round: apply ONLY the two rag.rs rewrites by hand, and explicitly NOT run bare
  `cargo fmt` (which would reformat the 8 pre-existing violations and trip the scope-drift
  gate). The worker fixed both lines (commit 3d51625); re-verify showed 0 rag.rs fmt
  violations, tests still green. The pre-existing debt was left untouched for a dedicated
  cleanup task.
