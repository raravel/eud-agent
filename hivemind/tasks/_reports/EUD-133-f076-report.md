---
task_id: EUD-133-f076
completed_at: 2026-06-09T22:17:37
duration_minutes: 75
coding_retries: 1
verify_retries: 1
review_rounds: 0
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
  coder_session_id: 019eabd9-a218-74b3-81c4-db56b3482050
  coder_tokens:
    input: 7747155
    output: 37103
    total: 7784258
  reviewer_tracked: false
---

## Summary
Rebuilt the vendored `native/isom` C++ engine on the dynamic C runtime (`/MD`) so
`isom_capi.lib` can be statically linked into `eud-agent` alongside `ort_sys` (the prebuilt
`/MD` ONNX runtime from `fastembed`). This implements Decision 14 and unblocks EUD-128.

Net change: `RuntimeLibrary` `/MT`->`/MD` (and `/MTd`->`/MDd`) across the 6 vcxproj built by
`isom_capi.sln`, plus removal of the static-CRT forcing in both `crates/isom-sys/build.rs` and
`crates/isom/build.rs`. **`WholeProgramOptimization` (/GL) was intentionally RETAINED** — see
Deviation below.

## Changes
- `native/isom/isom_capi.vcxproj`, `CascLib/CascLib_vs22.vcxproj`, `CrossCutLib/CrossCutLib.vcxproj`,
  `IcuLib/common.vcxproj`, `MappingCoreLib/MappingCoreLib.vcxproj`, `StormLib/StormLib_vs22.vcxproj`
  — `RuntimeLibrary` -> `MultiThreadedDLL` (Release/ReleaseUS/ReleaseAS) / `MultiThreadedDebugDLL`
  (Debug/DebugUS/DebugAS), all configs.
- `crates/isom-sys/build.rs` — removed `CRT_STATIC_LINK_ARGS` const + its `rustc-link-arg` loop;
  kept `rustc-link-search` + `rustc-link-lib=static=isom_capi` + SYSTEM_LIBS + bindgen; module
  doc CRT note updated to `/MD`.
- `crates/isom/build.rs` (scope-added, see Incident) — removed the mirror `CRT_STATIC_LINK_ARGS`
  + loop; kept the engine-archive search path + raw `isom_capi.lib` link-arg (still required: the
  `static=isom_capi` from the `links="isom_capi"` crate is deduplicated and never reaches isom's
  own test link); doc updated to `/MD`.
- harness: feature 13 Implementation bound `crates/isom/build.rs`; Decision 14 gained an Addendum
  documenting /GL retention.

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- Verify-first baseline: pre-implementation `cargo build -p eud-agent` (temp isom wiring, current
  `/MT`) FAILED `LNK1120` (5 unresolved `isom_*`) — failing state confirmed before work.
- `cargo build -p isom` — OK.
- `cargo test -p isom -- --include-ignored` — OK (3 lib + 3 ffi incl. `ffi_smoke` byte-identical
  round-trip). [criterion 3]
- Coexistence (temp src-tauri build.rs re-supplying `isom_capi.lib`, mirroring EUD-128 plumbing):
  `cargo build -p eud-agent` LINKS (30.6s, isom `/MD,/GL` + ort `/MD`, no LNK2019/LNK1120/LNK4098);
  `cargo test -p eud-agent mapsafe` 8 passed. Temp wiring reverted before commit. [criterion 4]
- `cargo clippy --workspace --all-targets -- -D warnings` — clean; `cargo fmt --manifest-path
  src-tauri/Cargo.toml -- --check` — clean. [criterion 5]

## Review
codex review (`--base main`) returned one finding: `[P2]` "disable /GL alongside the CRT switch"
(LTCG-archive toolchain fragility on lld-link / mismatched MSVC toolsets). **Overridden by the
orchestrator** as based on a false premise for this codebase: removing /GL breaks compilation
(vendor LTCG dependency, below) and is infeasible without forbidden vendor C++ edits; the
fragility hypothetical does not apply to the MSVC `link.exe` toolchain that rules.md mandates and
that the verification empirically links with. No other findings; the RuntimeLibrary/build.rs
changes were clean. Worker NOT looped (looping to remove /GL would reintroduce LNK2019).

## Deviation (criterion 1, /GL clause)
Criterion 1 also asked to "remove WholeProgramOptimization (/GL)". This was NOT done — it is a
measured spec error. `Chk::Action::stringUsed` / `briefingStringUsed` (MappingCoreLib) are
declared `inline` in `Chk.h` and defined `inline` in `Chk.cpp`, but ODR-used cross-TU from
`Scenario.cpp`; with no out-of-line definition emitted, a non-LTCG build leaves them unresolved
(`LNK2019`). Only /GL's whole-program cross-TU inlining resolves them. /GL is orthogonal to the
CRT, so the task GOAL (CRT coexistence, unblock EUD-128) is fully met by the `/MD` switch alone.
Decision 14 was corrected via Addendum. **User action: ratify the Decision 14 /GL correction via
/hv:plan if desired.**

## Notes
- Profile `mixed` specifies executor/reviewer `gpt-5.2-codex`, which this machine's ChatGPT-account
  codex rejects (HTTP 400 "not supported"). Fell back to the codex config default `gpt-5.5` for
  both coding and review. Consider updating the `mixed` profile to a supported model.
- criterion 4 cannot be fully validated within EUD-133's scope (it needs isom wired into src-tauri
  = EUD-128); validated here via temporary EUD-128-style link plumbing, reverted before commit.

## Incident

### What broke
1. `cargo test -p isom` failed (`LNK2019`/`LNK1120`) after the in-scope edits, because
   `crates/isom/build.rs` (OUT of the original scope) also emitted the static-CRT link args in
   lockstep with `isom-sys/build.rs`.
2. After dropping /GL (per criterion 1), `cargo test -p isom` failed with unresolved
   `Chk::Action::stringUsed` / `briefingStringUsed` (MappingCoreLib inline-in-.cpp functions that
   rely on LTCG cross-TU inlining).

### Why
1. The task scope (Decision 14) listed only `crates/isom-sys/build.rs`, missing the sibling
   wrapper build script that re-supplies non-transitive link args for isom's own test binaries.
2. Decision 14's "remove /GL" Impact bullet did not account for vendored MappingCoreLib code that
   is only well-formed under LTCG.

### What fixed it
1. Orchestrator ran `hv task scope-add EUD-133-f076 crates/isom/build.rs` (no in-flight peers ->
   disjoint), then resumed the codex coder session to remove the mirror CRT args. (coding_retries)
2. Restored `WholeProgramOptimization=true` (kept /GL); only RuntimeLibrary -> `/MD`. Re-verified:
   isom tests + eud-agent coexistence both pass. (verify_retries)
