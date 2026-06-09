---
task_id: EUD-129-a747
completed_at: 2026-06-09T23:05:00
coding_retries: 0
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
  coder_session_id: 019eaca5-b3d1-7790-a01e-c861be14d27e
  coder_tokens:
    input: 7005653
    output: 30982
    total: 7036635
  reviewer_tracked: false
---

## Summary
Registered and implemented the `map_info` READ tool in `src-tauri/src/tools.rs`. It resolves the
connected source map (`GETSET project|OpenMapName` via bridge_io), extracts the CHK in-process
(`isom::chk_extract`), parses it (`chk::digest_chk` — previously a dead-code path, now has its
first live caller), and slices the digest by `mode` (summary|locations|units|players) with
owner/unitType filters and a 200-entry cap on the units list. All slicing/filter/truncation logic
lives in a pure `map_info_view(digest, args, path, saved_at)` for headless unit testing.

## Changes
- `src-tauri/src/tools.rs` — `map_info` ToolSpec (read_only) + schema (mode enum, owner, unitType);
  `map_info(bridge, args)` handler (GETSET -> mtime -> chk_extract -> digest_chk -> view);
  pure `map_info_view` (mode slicing, unitsByOwner aggregation, owner/unitType filters, 200 cap +
  `truncated`, `map.{path,savedAt}` envelope); `map_info_error` -> ToolError; tests.

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- `cargo test -p eud-agent` — 106 passed, 0 failed, 1 ignored (map_info registration/READ +
  invalid-mode-rejected-before-counting, summary aggregates w/o raw units, locations/units/players
  shapes, 200 cap + truncated, owner numeric+substring+P12/neutral filters). [criteria 1,2,3]
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --manifest-path
  src-tauri/Cargo.toml -- --check` clean. [criterion 4]
- `chk::digest_chk` now has a live caller (map_info). [criterion 3]

## Review
codex review (`--base main`) returned one finding:
- [P2] map_info units `owner="P12"` filter matched nothing — neutral units are labeled
  `"P12 (neutral)"` by `chk::owner_label`, but the filter did an exact compare. REAL bug; fixed
  (one review round).

## Harness Sync
- No-op: `src-tauri/src/tools.rs` is already documented in features 05/07/11 `## Implementation`;
  no manifest change. (NOTE for user: feature 08_map-info-tool.md is still the v1/Python spec —
  references `chk_info.py`/`IsomTerrain.exe`/`tools.py` and has no `## Implementation` section.
  Worth a `/hv:plan` re-ground to the v2 Rust reality: `map_info` in `tools.rs`, `isom::chk_extract`,
  `chk::digest_chk`.)

## Notes
- Self-contained in tools.rs: OpenMapName came from the existing generic `BridgeIo::send` (the
  bridge `GETSET project|OpenMapName` command), so no bridge_io/bridge.lua change was needed.
- Model: profile `mixed` -> `gpt-5.2-codex` is rejected on this ChatGPT-account codex; used `gpt-5.5`.

## Incident

### What broke
- Code review [P2]: `map_info(mode="units", owner="P12")` returned zero units even though the schema
  advertises `P12` as a valid owner.

### Why
- `chk::owner_label` renders the neutral slot as `"P12 (neutral)"`, but the units owner filter
  compared the raw filter value (`"P12"`) for exact equality, so the suffix made it never match.

### What fixed it
- Replaced the exact compare with `unit_owner_matches_filter`: matches on exact equality, on a
  `"{filter} "` prefix (so `"P12"` matches `"P12 (neutral)"` while `"P1"` does NOT prefix-match
  `"P12 ..."`), or on `neutral` matching any `"(neutral)"` label. Added a regression test. Fixed on
  the single review round (codex exec resume of the coder session).
