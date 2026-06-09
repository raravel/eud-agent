---
task_id: EUD-115-7e3f
completed_at: 2026-06-10T01:20:00
coding_retries: 0
verify_retries: 1
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
  coder_session_id: 019ead12-4318-71c2-8eaf-5b74c4a9fe74
  coder_tokens:
    input: 3695870
    output: 41146
    total: 3737016
  reviewer_tracked: false
---

## Summary
Ported the Python project-memory store (`server/eud_agent/memory.py`) to Rust
`src-tauri/src/memory.rs`. Per-project store rooted at `%appdata%\eud-agent\memory\<sanitized>\`
(via `config::DataDirs::memory_dir()`): the four codex/panel markdown files
(resources/structure/conventions/lessons) with full-file replace + an 8192-byte cap,
`episodes.jsonl` (append-only), `meta.json` (list-hash staleness), atomic UTF-8-no-BOM writes, and
`render_section` building the `[project memory]` block with the documented truncation order (drop
episodes first, then tail-truncate lessons; 40000-char cap).

## Changes
- `src-tauri/src/memory.rs` (NEW) — `sanitize_project_name`, `list_hash` (sha256), `WriteResult`,
  `ProjectMemory` (enabled/store_dir/read/write/append_episode/read_episodes/read_meta/write_meta/
  update_list_hash/is_stale/render_section), atomic `write_atomic_bytes` with UNIQUE per-call temp
  paths; `#[cfg(test)]` unit tests.
- `src-tauri/src/lib.rs` — `pub mod memory;` (scope-added; one line).

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- `cargo test -p eud-agent memory` — 10 memory tests pass (round-trip under the memory root, over-cap
  + unknown-name rejection, sanitization + disabled-empty-name, episodes append/read, meta + is_stale,
  render order/staleness/episode-corrections, render truncation order, unique-temp-path) PLUS the two
  tools tests `memory_write_skips_mutation_gate_and_counter` /
  `evidence_gate_never_blocks_memory_write_or_build_run`. [criteria 1,2,3]
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --manifest-path
  src-tauri/Cargo.toml -- --check` clean. [criterion 4]

## Review
codex review (`--base main`) returned one finding:
- [P2] atomic-write helper used a deterministic `<file>.tmp`, so a panel save racing an agent
  `memory_write` could clobber the other's temp before rename (feature 07 allows both writers;
  the v2 app is multi-threaded). REAL; fixed (one review round).

## Harness Sync
- No-op: `src-tauri/src/memory.rs` is already listed in features/11_rust-backend-core.md
  `## Implementation` (and the `## memory` section describes the behavior); no manifest change.

## Notes
- Criterion 2 ("memory_write bypasses the evidence gate") is satisfied in tools.rs
  (`is_evidence_gate_exempt` matches `MEMORY_WRITE_TOOL`); this task ported the STORE only and did
  not touch tools.rs. v1's `<data_dir>/harness/<project>` root maps to v2's
  `DataDirs::memory_dir()` (`%appdata%\eud-agent\memory`). Model: profile `gpt-5.2-codex` is rejected
  on this ChatGPT-account codex; used `gpt-5.5`.

## Incident

### What broke
1. Verification: one `clippy::manual_pattern_char_comparison` warning (`-D warnings`) on the
   trailing dot/space strip in `sanitize_project_name`.
2. Review [P2]: deterministic atomic-write temp path races concurrent writers.

### Why
1. The port used a closure `trim_end_matches(|c| c == '.' || c == ' ')` where clippy prefers an array.
2. The temp path mirrored the Python `<file>.tmp` (safe-ish under the GIL/single-thread) but is racy
   in the multi-threaded Rust app.

### What fixed it
1. `trim_end_matches(['.', ' '])` (orchestrator-applied, re-formatted, re-verified).
2. Unique per-call temp name `<file>.<pid>.<nanos>.<seq>.tmp` (process-wide `AtomicU64` seq +
   SystemTime nanos), same directory so `rename` stays atomic; best-effort temp cleanup on error;
   regression test that successive temp paths differ and leave no stray `.tmp`. Fixed on the single
   review round (codex exec resume of the coder session).
