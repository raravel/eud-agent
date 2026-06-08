---
task_id: EUD-103-b1f5
completed_at: 2026-06-08T12:30:00
duration_minutes: 20
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 10
  safety: 9
  clarity: 9
tokens:
  estimated: true
  input: 169854
  output: 29974
cost_usd: 4.80
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Created `crates/isom-sys`: a `build.rs` that builds the vendored `isom_capi` MSBuild static
lib (ReleaseUS|x64), emits the link directives, and runs bindgen over `isom_capi.h`; a
`src/lib.rs` exposing the raw FFI with a `#[used]` link anchor. `cargo build/test -p isom-sys`
links the C lib and `isom_abi_version()` returns 1 from Rust. The safe wrapper + mapsafe rails
remain separate later tasks.

## Changes
- `crates/isom-sys/Cargo.toml` — `links = "isom_capi"`, build-dep `bindgen = "0.70"`.
- `crates/isom-sys/build.rs` — vswhere→MSBuild on `isom_capi.sln` (not the bare vcxproj, so
  `$(SolutionDir)` resolves and the 6 dep `.lib`s land in one OutDir); `strip_verbatim` for
  the `\\?\` prefix; pre-creates the ICU header-copy dest; emits `link-search` +
  `link-lib=static=isom_capi` + 10 Win32 dylibs + 4 static-CRT link-args; bindgen allowlist.
- `crates/isom-sys/src/lib.rs` — `include!` bindgen output; `#[used] __ISOM_LINK_ANCHOR` to
  keep `static=isom_capi` propagating; in-crate `#[cfg(test)]` smoke.
- `Cargo.toml` (root) — `members += "crates/isom-sys"`.
- `Cargo.lock` — bindgen + transitive deps locked.
- `.gitignore` (root) — `+ native/include/` (ICU header-staging build output, sibling of
  native/isom, previously untracked-non-ignored after every isom build).

## Verification (run directly by the orchestrator in the worktree)
- `cargo build -p isom-sys` → builds `native/isom/x64/ReleaseUS/isom_capi.lib` (80.7 MB) via
  build.rs's MSBuild, links it. Finished.
- `cargo test -p isom-sys` → `abi_version_is_one ... ok` (1 passed) — the bindgen
  `isom_abi_version()` links against the static lib and returns 1.
- `cargo clippy -p isom-sys --all-targets -- -D warnings` → exit 0.
Verify-first honored: test-first commit (`920a0dd`, failing `cannot find function
isom_abi_version`) preceded the impl commit (`85a2e68`).

## Completion Criteria
- [PASS] `cargo build -p isom-sys` builds the C lib and links it — verified (lib built fresh
  by build.rs; smoke links it).
- [PASS] bindgen FFI usable; `isom_abi_version()` callable — verified (test returns 1).
- [PASS] clippy passes — exit 0.

## Review
Reviewer (opus): no blocking findings; rubric 9/10/9/9. Confirmed the `#[used]` fn-pointer
anchor is sound and the idiomatic fix for the `-sys`-crate static-lib propagation gap; CRT
static-match (`/MT`) is the right call over rebuilding the C lib `/MD`; the `rustc-link-arg`
non-transitivity caveat is accurate and load-bearing for the downstream `src-tauri` link.

## Harness Sync
- Contract-drift guard: PASS (purely additive — new crate + workspace member + lockfile; no
  removed/renamed spec identifiers, no rule-contradicting comments).
- File bindings: `crates/isom-sys/build.rs` + `src/lib.rs` are already in feature 13
  `## Implementation` — no append (idempotent).
- Dep binding: `bindgen 0.70` is already listed in tech-stack.md `## Target Rust Stack` — no
  append (idempotent).

## Notes — downstream guidance for the `isom` wrapper + `src-tauri` link (EUD-104+/EUD-098-chain)
- **CRT args are NOT transitive.** build.rs emits `/NODEFAULTLIB:msvcrt.lib`,
  `/NODEFAULTLIB:msvcprt.lib`, `/DEFAULTLIB:libcmt.lib`, `/DEFAULTLIB:libcpmt.lib` via
  `cargo:rustc-link-arg`, which applies ONLY to this crate's own link. The `src-tauri` final
  link (the Tauri binary) MUST re-supply the same four args (its own build.rs or
  `.cargo/config.toml`), or set `-C target-feature=+crt-static` globally — else LNK2005/
  LNK4098 CRT mismatch. The Win32 `rustc-link-lib`s and `static=isom_capi`, by contrast, DO
  propagate transitively via the `links` key.
- **Build the `.sln`, not the `.vcxproj`** (the dep `.lib`s key off `$(SolutionDir)`).
- The first build is a cold ~10-15 min ICU/CascLib/StormLib MSBuild. `native/include/` (ICU
  headers) + `native/isom/x64/` are build output, now both gitignored.
- LNK4006 (StormLib+CascLib dup zlib) is benign; no `/FORCE:MULTIPLE` needed.
- An integration test (`tests/ffi_smoke.rs`) would NOT link (rustc prunes `static=` libs for
  integration-test binaries); the smoke is in-crate. Keep regression smokes in-crate.
- The agent worktree forked from the POC-era base `23bc6f4`; the worker rebased onto `main`
  (`a60a576`) before starting — clean merge-base.
