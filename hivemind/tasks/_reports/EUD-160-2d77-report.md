---
task_id: EUD-160-2d77
completed_at: 2026-06-12T17:55:00
duration_minutes: 6
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
  correctness: 10
  spec_compliance: 10
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 63576
  output: 11220
cost_usd: 1.27
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
Bumped the CI workflow's published RAG-index format default to **version 2**. The builder
(`ci/build_rag_index.rs`, EUD-156) already emits a v2 binary (`INDEX_VERSION = 2`) and the
`rag-index.bin.sha256` sidecar, and the workflow already publishes all three release assets,
so the only required change was the version FALLBACK default (`1`→`2`) used on an untagged /
no-input run, plus the matching `workflow_dispatch` input description text.

## Changes
- `.github/workflows/build-rag-index.yml` (2 lines):
  - `workflow_dispatch` `version` input description: "…tag suffix, then 1." → "…then 2."
  - "Generate release manifest" step fallback: `version="1"` → `version="2"`.
  Trigger globs, permissions, `runs-on: ubuntu-latest`, step order, caching, and the
  3-asset publish list (`rag-index.bin`, `rag-index.bin.sha256`, `rag-index.manifest.json`)
  are unchanged.

## Verification
Run on the merged tree:
- `python -c "import yaml; yaml.safe_load(open(...))"` → YAML OK; `runs-on` parsed as
  `ubuntu-latest`.
- `grep version="` → fallback is now `"2"` at line 66; no stray `"1"` default remains.
- Builder is already v2 (`ci/build_rag_index.rs` `INDEX_VERSION = 2`, writes `.sha256`
  sidecar) — the workflow runs it unchanged, so a no-input run now produces a v2 binary,
  a `version: "2"` manifest, and a `rag-index-v2` release tag.

Verify-first: the orchestrator first confirmed the fallback default was `"1"` (target
assertion failing) before the change; this single-file CI-YAML task has no in-repo
executable test surface (GitHub Actions cannot be unit-tested in-repo) and the scope is the
one workflow file, so the committed-test form of the gate is N/A — the gate was satisfied by
the before/after YAML-validity + version-content assertions run by the orchestrator.

## Review
Claude reviewer (claude-sonnet-4-6): **no blocking issues**. Rubric 10/10/9/10. Advisories
(non-blocking): the fallback runs on any untagged trigger — pre-existing behavior, not
introduced here (the `on:` block only fires on `ci/corpus/**` pushes or `rag-index-v*` tags);
no stale v1 literal elsewhere.

## Harness Sync
- Skipped (no-op): the only changed file `.github/workflows/build-rag-index.yml` is already
  listed under features/17_rag-knowledge-tiering.md ## Implementation; no manifest file
  changed. Contract-drift guard: only a version literal + description text changed — no
  removed/renamed identifier, no signature change, no rule-contradicting comment.

## Notes
- Worker worktree branched from a stale base (pre EUD-154..158); main never touched this
  workflow file since, so the squash-merge applied cleanly; verification run on the merged
  tree.
- Completing this task (the last child of story EUD-152-0709) auto-moved EUD-152 from
  active → done. Verified legitimate: all EUD-152 children (EUD-154..160) are done; the
  still-pending EUD-153/EUD-161 belong to the separate EUD-149 GPU-track story.
- Pre-existing uncommitted `src-tauri/Cargo.toml` modification left untouched / excluded.
