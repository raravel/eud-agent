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

### MainFile architecture focused checks

- `cargo test -p eud-agent bridge_io --lib` — verifies root and nested `GETMAIN` paths, empty/no
  project mapping, unexpected-error propagation, and the unchanged Lua `GETMAIN` identity walk
  plus `LIST` row shape.
- `cargo test -p eud-agent tool_exec --lib` — verifies additive
  `project_status.mainFile`, unchanged `status`, JSON `null`, read-only execution without a write
  lease, name/type independence, and shared `set_main` prior-value journaling.
- `cargo test -p eud-agent engine --lib` — verifies the canonical architecture guide in cold-start
  and resume prompts, section boundaries/order, MainFile non-inference, localized ownership,
  acyclic dependencies, complete `structure` refresh, one-batch preflight, and mandatory build.

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
- isom FFI (feature 13): `cargo test -p isom ffi_smoke -- --ignored` — CHK
  extraction, switch rename on a copied sample, re-extraction, and terrain render.
  `ISOM_SMOKE_MAP=<real map>` selects a richer render fixture and
  `ISOM_SMOKE_BMP_OUT=<path>` retains pixels for visual inspection.
- map switch write: `cargo test -p eud-agent switch_write_real_map_roundtrip -- --ignored`
  copies `sample.scx`, runs the full MapSafe/native/journal path, and requires the
  exact renamed SWNM string after re-extraction.
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
- Panel chat control/performance: `npm --prefix panel test -- --run App SessionSidebar ipc AskCard
  AgentAnswer ConversationLog PlanView store` covers edit-prefix truncation, cancel feedback,
  structured ASK routing/submission, Mermaid answer/plan rendering, live-stage labels, and a
  200-entry conversation mounting fewer than 50 viewport/overscan rows.

- Concurrent sessions: `cargo test -p eud-agent write_coordinator::tests`,
  `cargo test -p eud-agent workspace::tests::concurrent_session_accept`,
  `cargo test -p eud-agent reject_restores_one_session_while_another_writer_remains_active`,
  and `cargo test -p eud-agent pending_review_recovery_coexists_with_new_writer_ticket`.
- Panel concurrency: `npm --prefix panel test -- --run App SessionSidebar ipc` proves overlapping
  chat invokes, immutable event routing, simultaneous write/review labels, conflict reporting,
  and selection independence.
- Mock-Tauri browser smoke: put A in review while B is `running_write` and C remains idle.
  Select B and require `변경 중 · 격리 워크스페이스` plus the write-transition log. Repeat at
  1280×800 and 960×720 and require
  `document.documentElement.scrollWidth === document.documentElement.clientWidth`.
- ASK + Mermaid browser smoke: use Mock-Tauri to emit a two-question `ask` event, submit mixed
  single/multi/direct input, and require the exact same-session `ask_response` payload plus zero
  horizontal overflow. Emit fenced Mermaid in an archived answer and plan; require real Mermaid
  SVG nodes and no raw `pre/code` fallback.

## E2E (user-assisted, GUI)
- MainFile architecture live smoke: with `main` (`ClassicTrigger`, not MainFile) and
  `survivor_mvp` (`CUIEps`, configured MainFile), call `project_status` and require
  `mainFile == "survivor_mvp"`; call `list_files` and require both original paths/types. Ask a
  read-only architecture question and require `survivor_mvp` as composition root, imports from it
  in any modular proposal, and no `set_main` or other mutation. Do not modify the user's source or
  structure. Verify the unset case only with a fake bridge/isolated fixture unless clearing the
  live setting is separately safe and approved.
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
