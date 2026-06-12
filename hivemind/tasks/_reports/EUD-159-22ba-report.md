---
task_id: EUD-159-22ba
completed_at: 2026-06-12T17:47:51
duration_minutes: 12
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
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 163900
  output: 28900
cost_usd: 4.09
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
Made the first-run bootstrap require RAG index format **version 2**. Added
`REQUIRED_RAG_INDEX_VERSION = "2"` in `bootstrap.rs`; `needs_bootstrap` now reports a
re-install when the pinned `rag_index.version` is not v2 (even if the asset is present and
its sha256 matches), and `parse_release_manifest` rejects any manifest whose version is not
v2. The verified atomic + sha256 download path (`verify_and_place`, `asset_status`,
streaming download) is untouched. A one-line broadening in `setup.rs` re-fetches the release
manifest on a version mismatch so a stale v1 user actually upgrades end-to-end instead of
looping.

## Changes
- `src-tauri/src/bootstrap.rs` — new `pub const REQUIRED_RAG_INDEX_VERSION: &str = "2"`;
  `needs_bootstrap` gates on `version != REQUIRED_RAG_INDEX_VERSION`; `parse_release_manifest`
  bails on a non-v2 manifest version; existing manifest-parse test fixture updated v1→v2;
  3 new unit tests (stale-v1-present → re-download, present-v2 → no bootstrap, manifest
  requires-v2).
- `src-tauri/src/setup.rs` (scope-add) — `run_bootstrap_inner` re-fetch trigger broadened
  from `sha256.is_empty()` to also fire on `version != bootstrap::REQUIRED_RAG_INDEX_VERSION`;
  `place_rag_asset` test helper version pinned to the constant.

## Verification
Run on the merged tree (worker branch + main) with the shared cargo target cache:
- `cargo test --manifest-path src-tauri/Cargo.toml bootstrap` — 15 passed, 0 failed.
- `cargo test --manifest-path src-tauri/Cargo.toml setup` — 14 passed, 0 failed.
- `cargo test --manifest-path src-tauri/Cargo.toml` — full suite passed, 0 failed.
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` — clean.
- `cargo fmt -- --check` — the two changed files are rustfmt-clean; the crate-wide check
  reports diffs ONLY in pre-existing untouched files (`bridge_install.rs`, `engine.rs`,
  `journal.rs`, `tool_exec.rs`) — the documented main fmt debt, left untouched per the
  surgical-changes rule.

Verify-first gate: the worker added the 2 failing assertions first; the orchestrator
independently confirmed they failed on assertion (not compile) before implementation.

## Review
Claude reviewer (claude-sonnet-4-6): **no blocking issues**. Rubric 9/10/9/9. Advisories
(all non-blocking, by-design or cosmetic): an unversioned future manifest bails with a
slightly opaque `version "" is not the required "2"` message; a future v3 will be rejected
until the constant is bumped (intentional hard pin); the version predicate is duplicated
across `needs_bootstrap` and `run_bootstrap_inner` (a `rag_index_needs_refresh` helper could
dedup). The reviewer explicitly verified the v1→v2 upgrade loop is closed by the setup.rs
broadening.

## Harness Sync
- features/17_rag-knowledge-tiering.md += `src-tauri/src/setup.rs` (BOUND). `bootstrap.rs`
  was already listed under ## Implementation (no-op).
- No manifest (Cargo.toml/etc.) in the staged diff → no dep binding.
- Contract-drift guard: no removed/renamed function or endpoint identifiers, no signature
  changes, no comment-encoded rule contradicting rules.md → clean.

## Notes
- Scope-add: `hv task scope-add EUD-159-22ba src-tauri/src/setup.rs` was used. The setup.rs
  change is required for a correct end-to-end v1→v2 upgrade (a v1 user has a non-empty
  sha256, so the prior `sha256.is_empty()` re-fetch guard would never fire and
  `needs_bootstrap` would loop forever). Disjointness was trivially satisfied — sequential
  mode, no in-flight peers. The planner originally scoped only `bootstrap.rs`.
- Worker worktree was branched from a stale base (683def7, before EUD-154..158). main did
  NOT touch bootstrap.rs/setup.rs since that point, so the squash-merge applied cleanly with
  no overlap; verification was run on the merged tree to confirm.
- Orchestration gotcha (no task incident): `hv feedback draft-add` is deprecated and now
  delegates to `hv feedback save`, which runs `git commit` and swept the orchestrator's
  staged squash-merge into a `feedback: … [lesson:…]` commit. Recovered with
  `git reset --soft` and re-bundled into the proper single `task:` commit.
- Pre-existing uncommitted `src-tauri/Cargo.toml` modification (present at session start) was
  left untouched and excluded from the task commit.
