---
task_id: EUD-105-dba3
completed_at: 2026-06-09T16:46:00
duration_minutes: 52
coding_retries: 0
verify_retries: 2
review_rounds: 1
verification_required: true
verification_passed: true
blocking_issues: false
providers:
  coder: codex
  reviewer: codex
review_scores: {}
tokens:
  estimated: false
  input: 1325642
  output: 90082
cost_usd: 0.00
profile: mixed
models:
  executor: gpt-5.2-codex
  reviewer: gpt-5.2-codex
codex_usage:
  coder_session_id: null
  coder_tokens:
    input: 1325642
    output: 90082
    total: 1415724
  reviewer_tracked: false
---

## Summary
Ported the CHK parsing logic of `server/eud_agent/chk_info.py` into a new Rust module
`src-tauri/src/chk.rs`: the signed-size TLV section walk, duplicate resolution (UNIT stacks,
others last-win), STR/STRx string-table decode (utf-8 → cp949 → latin-1, total), and the
section decoders for MRGN locations, UNIT units + start locations (type 214), and
OWNR/SIDE/FORC players + forces, assembled by `digest_chk` into the same structured digest
(map / players / forces / locations / units / startLocations) the Python `map_info` produced.
Output structs serialize (serde) to byte-identical JSON to the Python contract.

Provider routing was the `mixed` profile (coder + reviewer = codex / gpt-5.2-codex), sequential.
Verify-first honored: Step A wrote serde structs + `unimplemented!()` stubs + the full test
module (9 tests RED), Step B implemented the bodies (9 GREEN).

## Changes
- `src-tauri/src/chk.rs` (NEW, ~1380 lines) — full parser port + inlined canonical `UNIT_NAMES`
  table (228 entries copied verbatim from `data/unit_names.json`) + `#[cfg(test)]` suite.
- `src-tauri/src/lib.rs` — `pub mod chk;` (module registration; scope-added).
- `src-tauri/Cargo.toml` — `encoding_rs = "0.8"` for cp949/EUC-KR string decode (scope-added).
- `Cargo.lock` — encoding_rs promoted to a direct dependency (was already transitive; scope-added).

## Verification
Run by the orchestrator (worker is sandboxed; shared `CARGO_TARGET_DIR` warmed in background
during codegen). panel/dist copied into the worktree for `tauri::generate_context!`.
- Verify-first (Step A): `cargo test chk` → 9 tests RED (`unimplemented!()` panics). Confirmed.
- `cargo fmt -- --check` → clean (orchestrator normalized; worker cannot run rustfmt).
- `cargo clippy --all-targets -- -D warnings` (eud-agent pkg) → 0 (after verify-fix 1).
- `cargo test chk` → **9 passed** (TLV EOF clamp / negative-size seek / cap, UNIT stacking,
  cp949 + STRx decode, MRGN zero-skip + anywhere + inverted, UNIT type-214 start locs, FORC
  short-padding + active-controller membership + flag bits).
- **Differential vs Python (the completion oracle)**: orchestrator built representative CHK
  byte blobs in Python, ran them through BOTH `chk_info.digest_chk` and the Rust
  `eud_agent_lib::chk::digest_chk` (throwaway integration harness, deleted before commit), and
  deep-compared the serialized JSON. After fixes: **IDENTICAL on two independent blobs** —
  blob 1 (DIM/ERA/STRx/MRGN/UNIT/OWNR/SIDE/short-FORC; inverted-x, anywhere, cp949 name) and
  blob 2 (unit id 23, zero string offset, all four force-flag bits, tileset "ice").
- `UNIT_NAMES` table compared element-wise to `data/unit_names.json`: **228/228 exact match**.

## Review
Codex review (`codex review --base 83a5ccb`) returned two blocking findings — both REAL parity
bugs that the differential's first blob had not covered, fixed in the single review round:
- **[P2] UNIT_NAMES diverged from canonical json** (ID 23 `Siege Tank` vs `Tank Mode`, dozens
  more) — the worker had typed the table from memory. Fixed by copying `data/unit_names.json`
  verbatim; re-verified 228/228 and via a differential blob containing unit id 23.
- **[P2] STR/STRx zero offset not treated as empty** (`off == 0` decoded the offset table as a
  name; Python uses `0 < off < len`). Fixed; re-verified via a hand-crafted zero-offset blob
  (location name → "" in both Python and Rust).

## Harness Sync
- `src-tauri/src/chk.rs` → already in features/13 `## Implementation` (no-op).
- `src-tauri/src/lib.rs` → already in features/10 & 11 (no-op); change is purely additive
  (`+pub mod chk;`) — no contract drift.
- tech-stack.md `## Active Dependencies` += `encoding_rs 0.8` (BOUND, auto-promoted).

## Notes
- Scope was expanded by the orchestrator (sequential mode, no in-flight peers → disjoint
  trivially) via `hv task scope-add` to include the integration points the port needs but the
  `chk.rs`-only scope could not hold: `src-tauri/src/lib.rs` (`mod chk;`), `src-tauri/Cargo.toml`
  + `Cargo.lock` (encoding_rs). These were applied by the orchestrator, not the sandboxed worker.
- `encoding_rs::EUC_KR` is the correct cp949 decoder: the WHATWG euc-kr index is the unified
  hangul code (Windows-949 / cp949), so it round-trips Korean SCMDraft names identically to
  Python's `cp949` codec — confirmed by the differential.
- Codex coder tokens are exact (Step A/B + 3 fix turns, `--json` summed); reviewer not
  token-tracked; no codex pricing entry → `cost_usd` 0.00 (billed separately).

## Incident

### What broke
- clippy `-D warnings` failed on the test helper `unit_entry` (`too_many_arguments`, 8/7).
- `digest_chk` output DIVERGED from Python on several constant/string tables, invisible to the
  worker's own tests (it wrote impl + expected values to the same wrong tables): tileset names
  capitalized (`Jungle` vs `jungle`), FORC flag key `allied` vs `allies`, empty-force-name
  fallback missing (`""` vs `Force N`), OWNR controller names wrong + id 8 missing.
- Review then found the inlined `UNIT_NAMES` table diverged from the canonical json (id 23+),
  and `parse_strings` let a zero offset through (control-char name instead of "").

### Why
- The worker reproduced string/mapping tables (unit names, controller/race/tileset names, flag
  keys) from model priors instead of copying the canonical Python source / json verbatim.
  Author-written unit tests encoded the SAME wrong constants, so they passed and masked the
  divergence — a self-consistency trap.

### What fixed it
- Verify-fix 1: `#[allow(clippy::too_many_arguments)]` on the fixed-arity byte-builder test helper.
- Verify-fix 2 (orchestrator differential, retry 2): a Python↔Rust differential on identical CHK
  bytes surfaced the table divergences; worker reset every table to chk_info.py verbatim.
- Review round 1: copied `data/unit_names.json` verbatim (228/228) and made `0 < off < len` the
  zero-offset empty condition. Re-verified with an extended differential blob.
