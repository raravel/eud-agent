---
task_id: EUD-104-0ae7
completed_at: 2026-06-08T12:55:00
duration_minutes: 16
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 10
  spec_compliance: 10
  safety: 9
  clarity: 10
tokens:
  estimated: true
  input: 134998
  output: 23823
cost_usd: 3.81
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
Created `crates/isom`: the safe `Result`-returning wrapper over the raw `isom-sys` FFI.
`chk_extract`/`locedit`/`playeredit`/`abi_version` with a typed `IsomError`; the one
C-allocated buffer is freed via `isom_free` on every exit path (RAII guard) — no leak, no
double-free. The map-safety rails remain a separate `mapsafe` task.

## Changes
- `crates/isom/Cargo.toml` — `isom-sys` (path dep) + `thiserror`.
- `crates/isom/src/lib.rs` — `IsomError` (each `IsomStatus` + `UnknownCode` + `NulError →
  InvalidArg`); `CBuf` RAII guard freeing the chk buffer once on any path; `chk_extract`
  (copy-out then free), `locedit`/`playeredit` (RAW `ops`, no re-encode), `abi_version`; 3
  unit tests.
- `crates/isom/build.rs` — re-supplies the native link directives that are NOT transitive to
  this crate's test binaries: the engine archive as a RAW `rustc-link-arg=<path>\isom_capi.lib`
  (a `rustc-link-lib=static=isom_capi` is DEDUPED away — the name is owned by isom-sys's
  `links` key — and never reaches the integration-test link → LNK2001) + the 4 static-CRT
  link-args. msvc-gated.
- `crates/isom/tests/ffi_smoke.rs` + `tests/fixtures/sample.scx` (22 KB real EUD map).
- `Cargo.toml` (root) — `members += "crates/isom"`; `Cargo.lock`.

## Verification (run directly by the orchestrator in the worktree)
- `cargo test -p isom` (non-ignored) → unit (status mapping/unknown/NUL) + integration
  (`abi_version_is_one`, `embedded_nul_path_is_invalid_arg`) all green.
- `cargo test -p isom ffi_smoke -- --ignored` → **ok — extracted a non-empty CHK (177,348
  bytes) from the real `sample.scx`** through the full C ABI.
- `cargo clippy -p isom --all-targets -- -D warnings` → exit 0.
Verify-first honored: failing-test commit (`347e509`) preceded the impl commit (`28196d1`).

## Completion Criteria
- [PASS] Safe chk_extract/locedit/playeredit with no leaks — `CBuf` guard frees `isom_free`
  exactly once on success / `?`-error / panic (no `panic=abort` in the workspace); reviewer
  confirmed sound.
- [PASS] `cargo test -p isom ffi_smoke -- --ignored` passes on a sample map — verified.
- [PASS] clippy passes — exit 0.

## Review
Reviewer (opus): no blocking findings; rubric 10/10/9/10. Confirmed the leak/double-free/panic
safety, exhaustive status mapping, and RAW-ops passthrough. Advisories (non-blocking): the
build.rs CRT/archive link constants are now copy-pasted across isom-sys/isom (and next
src-tauri) build scripts — a lockstep-fragility worth a shared source; non-UTF-8 path conflates
to InvalidArg (minor, Windows-only app).

## Harness Sync
- Contract-drift guard: PASS (additive — new crate + workspace member + lockfile).
- `crates/isom/src/lib.rs` is already in feature 13 `## Implementation`; `crates/isom/build.rs`
  is link-resupply glue thoroughly documented in this report + its own header; `thiserror` is
  already in tech-stack.md. No new doc binding required.

## Notes — downstream guidance (critical for `src-tauri`)
- **`src-tauri` final link will hit LNK2001 + CRT mismatch unless its own build.rs re-supplies**
  the engine archive as a RAW `rustc-link-arg=<...>\isom_capi.lib` AND the 4 static-CRT args
  (`/NODEFAULTLIB:msvcrt.lib`, `/NODEFAULTLIB:msvcprt.lib`, `/DEFAULTLIB:libcmt.lib`,
  `/DEFAULTLIB:libcpmt.lib`). `rustc-link-arg` is non-transitive, and `rustc-link-lib=static`
  for the `links`-owned name is deduped away. The Win32 dylibs DO propagate.
- Consider a shared `isom-link` helper (build-dep) or `DEP_ISOM_CAPI_*` metadata so the
  config/platform/CRT constants live in ONE place instead of being mirrored across 3 build
  scripts (reviewer advisory).
