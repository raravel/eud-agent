---
task_id: EUD-128-daea
completed_at: 2026-06-09T22:40:00
duration_minutes: 18
coding_retries: 0
verify_retries: 0
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
  coder_session_id: 019eac90-bd45-7ac3-8f9f-f9222d7070a2
  coder_tokens:
    input: 1624350
    output: 13409
    total: 1637759
  reviewer_tracked: false
---

## Summary
Wired the production isom-backed `MapEngine` into `src-tauri`, completing the map-write rails'
real backing. Adds the `isom` path dependency, an `IsomEngine` that routes writes to
`isom::locedit`/`isom::playeredit` and digests via `isom::chk_extract`, threads an `OpKind`
discriminator through `MapSafe::write` / `MapEngine::apply`, and adds the `src-tauri/build.rs`
plumbing that re-supplies the `isom_capi.lib` link directive (the piece that originally blocked
this task before EUD-133). `eud-agent` now links with isom + ort coexisting. Re-attempt after
EUD-133 resolved the CRT conflict.

## Changes
- `src-tauri/Cargo.toml` — `isom = { path = "../crates/isom" }`.
- `Cargo.lock` — isom/isom-sys entries (cargo-regenerated).
- `src-tauri/build.rs` — re-supply `isom_capi.lib` search path + raw `rustc-link-arg` (mirror of
  `crates/isom/build.rs`; rustc dedups the `links="isom_capi"` static-lib directive away from the
  final binary), alongside `tauri_build::build()`.
- `src-tauri/src/mapsafe.rs` — `OpKind { Locedit, PlayerEdit }`; `MapEngine::apply(map, kind, ops)`;
  production `IsomEngine` (digest -> chk_extract, apply -> locedit/playeredit, IsomError->String);
  `MapSafe::write(map, kind, ops)`; `FakeEngine` gains `last_kind` + new signature; all existing
  mapsafe tests updated; new `write_routes_opkind_to_engine` routing test.

## Verification (orchestrator-run, shared CARGO_TARGET_DIR)
- `cargo build -p eud-agent` — LINKS (1m20s; isom `/MD,/GL` + ort `/MD`, no LNK2019/LNK1120). [criterion 1]
- `cargo test -p eud-agent mapsafe` — 9 passed incl. `write_routes_opkind_to_engine`. [criterion 3]
- `cargo clippy --workspace --all-targets -- -D warnings` — clean; `cargo fmt --manifest-path
  src-tauri/Cargo.toml -- --check` — clean. [criterion 4]
- IsomEngine impl + IsomError mapping confirmed by reading the diff. [criterion 2]

## Review
codex review (`--base main`) returned NO findings: "consistently threads OpKind through MapSafe and
adds the production IsomEngine without an evident correctness regression; the extra link directive
mirrors the existing isom linking workaround and is scoped to the src-tauri package build."

## Harness Sync
- features/13_isom-ffi.md += `src-tauri/build.rs` (BOUND). `src-tauri/src/mapsafe.rs` already bound.
- No external dep added (isom is an internal path crate) — no tech-stack change.

## Notes
- Reused the prior (EUD-128 first-attempt) verified mapsafe wiring as the implementation reference;
  the codex worker reproduced it faithfully and added the new `src-tauri/build.rs` plumbing.
- Scope correction: the task scope listed `src-tauri/Cargo.lock`, but the workspace lockfile is the
  ROOT `Cargo.lock`; orchestrator `scope-add Cargo.lock`. The worker initially removed the
  cargo-added isom line from `Cargo.lock` thinking it out-of-scope; the orchestrator build
  regenerated it correctly.
- The sandboxed worker could not run the native isom MSBuild (`cargo build/test` failed at the
  isom-sys custom build step inside `-s workspace-write`); per the orchestrator model, all build /
  test / clippy verification was run by the orchestrator (non-sandboxed). Model: profile `mixed`
  specifies `gpt-5.2-codex` (rejected on this ChatGPT-account codex); used `gpt-5.5`.
