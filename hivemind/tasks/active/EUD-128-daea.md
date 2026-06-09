---
blocked_reason: 'build blocked: isom_capi.lib (/MT,/GL static CRT) vs ort_sys (/MD
  dynamic CRT, from fastembed) cannot coexist in one binary; cargo build/test -p eud-agent
  cannot link without /MD rebuild of native/isom/** vcxproj + crates/isom-sys/build.rs
  CRT change, outside the declared scope (src-tauri/**). Worker code (OpKind threading
  + IsomEngine) is correct. Re-plan CRT reconciliation as its own task.'
created: '2026-06-09'
depends_on: []
id: EUD-128-daea
parent: EUD-127-e1a7
priority: high
scope:
- src-tauri/src/mapsafe.rs
- src-tauri/Cargo.toml
- src-tauri/Cargo.lock
status: blocked
title: 'isom-backed MapEngine: wire mapsafe to isom FFI (locedit/playeredit/chk_extract)'
type: task
updated: '2026-06-09'
---

## Description
Provide the production `MapEngine` implementation that backs the map-write rails. `mapsafe.rs`
(EUD-106) defines the `MapEngine` trait (`apply` / `digest`) and all rails (backup, lock probe,
compiling guard, all-or-nothing, re-digest, journal/rollback, #64 protection) but ships ONLY a
`FakeEngine` — there is no real engine and the `isom` crate is not even a dependency of
`src-tauri`, so `chk.rs::digest_chk` and the map tools have no live backing.

- Add `isom = { path = "../crates/isom" }` to `src-tauri/Cargo.toml` (update `Cargo.lock`).
- Implement `IsomEngine: MapEngine`: `digest(map)` -> `isom::chk_extract(map)`; writes ->
  `isom::locedit` / `isom::playeredit`. Map `IsomError` -> `MapSafeError`/`String`.
- Thread an `OpKind { Locedit, PlayerEdit }` discriminator through `MapSafe::write(map, kind, ops)`
  and `MapEngine::apply(map, kind, ops)` so the engine routes to the correct isom call (the ops
  buffers reuse the MapGenCli locedit/playeredit encoding per feature 13). Update the existing
  `FakeEngine` + mapsafe tests for the new signature.

## Spec References
- [[features/13_isom-ffi|13_isom-ffi]] `../docs/features/13_isom-ffi.md` — C ABI ops, mapsafe rails, locedit/playeredit op encoding
- [[rules]] `../docs/rules.md` — map-write safety (autoDefragmentLocations=false, #64 protected, backup-before-write, no-share lock probe)

## Completion Criteria
- [ ] `isom` is a `src-tauri` dependency (Cargo.toml + Cargo.lock); `cargo build -p eud-agent` links
- [ ] `IsomEngine` implements `MapEngine` (digest -> chk_extract, apply -> locedit/playeredit by OpKind) with IsomError mapping
- [ ] `MapSafe::write`/`MapEngine::apply` carry an `OpKind`; existing FakeEngine + mapsafe unit tests updated and green
- [ ] `cargo test -p eud-agent mapsafe` passes; `cargo clippy --all-targets -- -D warnings` and `cargo fmt -- --check` clean