---
task_id: EUD-154-91b9
completed_at: 2026-06-12T15:28:06Z
duration_minutes: 14
coding_retries: 0
verify_retries: 0
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: true
providers:
  coder: claude
  reviewer: claude
review_scores:
  correctness: 6
  spec_compliance: 9
  safety: 8
  clarity: 8
tokens:
  estimated: true
  input: 202000
  output: 51000
cost_usd: 6.20
profile: balanced
models:
  executor: opus
  reviewer: claude-sonnet-4-6
---

## Summary
Added a resident L1 `EPS_IDIOMS` section to the system prompt in `src-tauri/src/engine.rs`.
It is a positive, example-bearing eps-idiom cheat-sheet (~1050 tokens, 12 idioms) that always
sits between `[first principles]` (L0) and `[reference context]` (L2) in BOTH
`build_system_prompt` and `resume_turn_text`. This is the "write eps like THIS" anchor that
keeps the model from falling back to SCMDraft classic triggers when retrieval misses. Idioms
that border a known crash cause cross-reference the `first_principles.md` item number rather
than restating the prohibition.

## Changes
- `src-tauri/src/engine.rs` — `EPS_IDIOMS` const + emit in `build_system_prompt` and
  `resume_turn_text` (after L0, before L2) + new ordering test
  `system_prompt_orders_eps_idioms_between_first_principles_and_reference_context`.

## Verification
- Verify-first gate: worker added a failing ordering test (commit 0688866); orchestrator
  confirmed it failed on assertion only (crate compiled).
- `cargo test --lib engine::tests` — 16 passed, 0 failed (incl. the new eps_idioms ordering
  test and the pre-existing `system_prompt_orders_first_principles_before_reference_context`
  and resume-invariant tests).
- `cargo clippy --all-targets -- -D warnings` — clean.
- Base check: worker worktree forked from stale base 683def7, but main changed only
  `tool_exec.rs` since then (engine.rs identical), so the squash-merge brought engine.rs
  cleanly with no conflict/rebase.
- Scope: worker touched only `src-tauri/src/engine.rs` (matches declared scope; no drift).

## Review
Sonnet review scored correctness 6 (blocking, <7). Two findings examined:
- `const ptr = f_dwread_epd(EPD(0x628438))` First-Empty-Unit example implied compute-once
  reuse — REAL issue. Fixed (commit 5fb109b) to `var ptr` re-read immediately before each
  CreateUnit, with explicit "never cached/hoisted across creates" wording.
- `unitType @ CUnit+0x64` low-16-bit masked compare flagged as possibly wrong — REVIEWER
  ERROR. The offset/mask matches the community-measured value in `first_principles.md`; left
  unchanged. spec_compliance 9, safety 8, clarity 8 all passed.
After the fix, correctness concern resolved (misleading example removed; the offset was
already correct). Re-verified: 16 tests pass, clippy clean.

## Notes
- Pre-existing repo-wide `cargo fmt --check` drift exists at HEAD (`bridge_install.rs:25`,
  `journal.rs:1243`, `tool_exec.rs:466`, and unrelated spots in `engine.rs`). Confirmed it
  predates this task (present on main HEAD with this change stashed); NOT introduced here and
  left untouched per surgical-changes rule. The `verify.md` lint stage `cargo fmt --check`
  will flag these independently — a separate cleanup, not part of EUD-154.
- `src-tauri/Cargo.toml` carries an unrelated uncommitted `M` from before this session; not
  included in the task commit.

## Incident

### What broke
- Review correctness axis scored 6/10 (below the <7 blocking threshold): the First-Empty-Unit
  eps example used `const ptr = ...`, implying the First-Empty-Unit address could be read once
  and reused, when it must be re-read immediately before every `CreateUnit`.

### Why
- The idiom was written for brevity as a single inline statement; `const` (compute-once) is
  the wrong binding for a value that must be refreshed per create. A model copying the pattern
  could cache a stale slot address and write to the wrong unit.

### What fixed it
- Round 1: rewrote the bullet to `var ptr = f_dwread_epd(EPD(0x628438))` shown in a per-create
  / loop-body context with explicit "re-read every time, never cached/hoisted across creates"
  wording (commit 5fb109b). Re-verified green.
