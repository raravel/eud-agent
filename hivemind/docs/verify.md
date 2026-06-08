# eud-agent Verify (v2 — Tauri + Rust)

What the orchestrator runs to confirm a task is complete. Commands are Windows/PowerShell.
Rust stages activate once `src-tauri/` exists; panel stages are live today. The Python
stages from v1 are retired as `server/` is removed.

## lint
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — Rust formatting is clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — no clippy warnings across the
  Rust workspace (`src-tauri` + `crates/isom-sys` + `crates/isom`).
- `cd panel && npx tsc -b --noEmit` — panel TypeScript typechecks (no separate eslint).

## type
- Covered by `cargo clippy` (Rust is type-checked at compile) and `tsc -b` above. No
  additional step.

## test
- `cargo test --workspace` — Rust unit + integration tests (ipc protocol, bridge_io file
  round-trip, codex fenced-block extraction, rag cosine ranking, mapsafe rails, chk parse).
- `cd panel && npx vitest run` — panel component/unit tests (PlanView, transport client).

## build
- `cd panel && npm run build` — `tsc -b && vite build` produces `panel/dist`.
- `cargo build --manifest-path src-tauri/Cargo.toml` — Rust core compiles **and links the
  isom static lib** (proves the FFI + MSBuild integration). On a release/packaging task:
  `cargo tauri build` produces the bundled exe.

## smoke (task-specific, run when the touched area supports it)
- RAG parity (feature 12): `cargo test -p eud-agent rag::parity -- --ignored` — top-k for a
  fixed query set matches the Python `sentence-transformers` baseline within tolerance.
- isom FFI (feature 13): `cargo test -p isom ffi_smoke -- --ignored` — chk extract on a
  sample map returns a parseable CHK; a no-op locedit round-trips byte-identical.
- bootstrap (feature 10): `cargo test -p eud-agent bootstrap::manifest` — missing/corrupt
  asset triggers re-download; sha256 mismatch refuses to install.

## E2E (user-assisted, GUI)
- Editor live test: install the slim bridge, launch the editor + app, run an instruct →
  apply cycle, confirm SET/NEWEPS land and the diff renders. Documented in the task; not
  headless.
