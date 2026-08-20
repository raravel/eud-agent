# eud-agent Verify (v2 — Tauri + Rust)

What the orchestrator runs to confirm a task is complete. Commands are Windows/PowerShell.
Rust stages activate once `src-tauri/` exists; panel stages are live today. The Python
stages from v1 are retired as `server/` is removed.

## lint
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` — Rust formatting is clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — no clippy warnings across the
  Rust workspace (`src-tauri` + `crates/isom-sys` + `crates/isom`).
- `npm --prefix tools/epscript-lsp-agent ci` — installs exactly the adapter build lock.
- `cd panel && npx tsc -b --noEmit` — panel TypeScript typechecks (no separate eslint).

## type
- Covered by `cargo clippy` (Rust is type-checked at compile) and `tsc -b` above. No
  additional step.

## test
- `cargo test --workspace` — Rust unit + integration tests (ipc protocol, bridge_io file
  round-trip, codex fenced-block extraction, rag cosine ranking, mapsafe rails, chk parse).
- `npm --prefix tools/epscript-lsp-agent test` — exercises syntax, ANTLR imports,
  transitive/reverse graph closure, Unicode, caps, cycles, process framing, and failure
  containment against the committed bundle.
- `cd panel && npx vitest run` — panel component/unit tests (state/IPC, cancel+rewind,
  persistent activity status, long-chat DOM virtualization, PlanView/ChangesetView).

## build
- `cd panel && npm run build` — `tsc -b && vite build` produces `panel/dist`.
- `cargo build --manifest-path src-tauri/Cargo.toml` — Rust core compiles **and links the
  isom static lib** (proves the FFI + MSBuild integration).
- Release/packaging: `cargo tauri build` produces the NSIS bundle. Inspect its resources
  for `adapter.cjs`, `adapter.sha256`, `LICENSE.md`, and `provenance.json`; no Node runtime
  is bundled.

## smoke (task-specific, run when the touched area supports it)
- RAG parity (feature 12): `cargo test -p eud-agent rag::parity -- --ignored` — top-k for a
  fixed query set matches the Python `sentence-transformers` baseline within tolerance.
- isom FFI (feature 13): `cargo test -p isom ffi_smoke -- --ignored` — chk extract on a
  sample map returns a parseable CHK; a no-op locedit round-trips byte-identical.
- bootstrap (feature 10): `cargo test -p eud-agent bootstrap::manifest` — missing/corrupt
  assets trigger re-download, sha256 mismatch refuses installation, and Codex release
  metadata must provide a same-tag CLI + Code Mode host + Windows sandbox setup helper set
  with valid digests.
- epScript adapter reproducibility (feature 18):
  `powershell -ExecutionPolicy Bypass -File scripts/build_epscript_lsp_agent.ps1 -Verify`
  downloads only the pinned archive, verifies its archive SHA-256, runs the exact lock,
  rebuilds, and fails unless bundle bytes and committed checksum are identical.
- epScript headless behavior (feature 18):
  `npm --prefix tools/epscript-lsp-agent test` drives the real framed adapter process over
  `main.eps -> lib/units.eps -> lib/common.eps`, mutually dependent candidates, imported
  syntax attribution, corrected re-check, and nested `EUDLSP001`.
- Rust/bridge preflight behavior (feature 18):
  `cargo test -p eud-agent eps_` covers fragmented/multiple/invalid frames, normalized
  paths, manifest ownership/containment, atomic overlays, mirror reuse/switching,
  stable skipped reasons, one-retry/reaping, non-journaling, budgets, and Lua Tick
  heartbeat/status/compiling invariants.
- Project workspace: `cargo test -p eud-agent workspace:: --lib` covers stable identity,
  durable document directories, coherent `source/` refresh, path confinement, `.codegraph`
  runtime-metadata exclusion, UTF-8/size limits, turn baseline diffing, Workspace journal
  kinds, restore behavior, exact approved-plan persistence/state, and linked
  spec-index/topic/worklog completion checks.
- Windows sandbox probe: with the installed Codex version, verify `eud_workspace_read`
  can read the session root but cannot write it, and `eud_workspace_write` can write documents
  while `source/**` remains read-only. Both profiles must deny unrelated user reads, outside
  writes, and network. Unsupported exact-root setup must fail closed.
- Panel workspace: `npm --prefix panel test -- --run WorkspaceView ChangesetView ipc`
  covers explorer/viewer states, canonical wiki-home ordering, safe relative Markdown
  navigation, approved-plan badges, confined IPC shapes, and category `workspace` diffs.
- Panel chat control/performance: `npm --prefix panel test -- --run ConversationLog InstructionBox store ipc`
  covers edit-prefix truncation, cancel feedback, live-stage labels, and a 200-entry
  conversation mounting fewer than 50 viewport/overscan rows.

- Concurrent sessions: `cargo test -p eud-agent different_session_read_turns_overlap_and_short_turn_finishes_first`,
  `cargo test -p eud-agent write_coordinator::tests`,
  `cargo test -p eud-agent opening_one_session_request_does_not_clear_another_sessions_state`,
  and `cargo test -p eud-agent workspace::tests::session_`.
- Panel concurrency: `npm --prefix panel test -- --run App SessionSidebar ipc` proves overlapping
  chat invokes, immutable event routing, backend activity labels, write-wait cancellation, and
  selection independence.
- Mock-Tauri browser smoke: use five sessions. Keep A in a long read while B completes; put C in
  review; complete D read during C review; show E as `쓰기 대기 1`; reject C and observe E enter
  `변경 중` only after rollback. Repeat at 1280×800 and 960×720 and require
  `document.documentElement.scrollWidth === document.documentElement.clientWidth`.

## E2E (user-assisted, GUI)
- Editor live test: install the slim bridge, launch the editor + app, and run an agent
  coordinated `.eps` change. Observe one complete-batch `eps_check`, correction/re-check
  before writes, then mandatory `build_run`. Repeat with Node unavailable and confirm a
  `skipped` result while write/build remain usable. This is the only Feature 18 scenario
  that requires the EUD Editor GUI; all adapter/fake-bridge/process coverage is headless.
- Workspace live test: the first project turn may request one elevated Windows sandbox
  setup. Approve it, approve a plan, and confirm `plans/<request-id>.md` appears immediately
  with an approved revision. Let Codex finish: `specs/index.md`, at least one linked topic
  spec, and a linked `worklog/<request-id>.md` must join the implementation changeset.
  Follow the index link inside the explorer. Reject once and verify implementation-authored
  specs/worklog restore while the approved plan remains; repeat and accept, restart the app,
  and verify the viewer retains accepted documents while `source/` refreshes from the editor.
