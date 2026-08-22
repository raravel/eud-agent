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
  structured ASK routing/submission and missed-event restoration through `ask_pending`, Mermaid
  answer/plan rendering, live-stage labels, and a 200-entry conversation mounting fewer than 50
  viewport/overscan rows.

- Concurrent sessions: `cargo test -p eud-agent write_coordinator::tests`,
  `cargo test -p eud-agent workspace::tests::concurrent_session_accept`,
  `cargo test -p eud-agent reject_restores_one_session_while_another_writer_remains_active`,
  `cargo test -p eud-agent pending_review_recovery_coexists_with_new_writer_ticket`,
  `cargo test -p eud-agent missing_pending_review_does_not_block_valid_review_recovery`, and
  `cargo test -p eud-agent rewind_clears_only_an_unrecoverable_pending_review`.
- Panel concurrency: `npm --prefix panel test -- --run App SessionSidebar ipc` proves overlapping
  chat invokes, immutable event routing, simultaneous write/review labels, conflict reporting,
  and selection independence.
- Mock-Tauri browser smoke: put A in review while B is `running_write` and C remains idle.
  Select B and require `변경 중 · 격리 워크스페이스` plus the write-transition log. Repeat at
  1280×800 and 960×720 and require
  `document.documentElement.scrollWidth === document.documentElement.clientWidth`.
- ASK + Mermaid browser smoke: use Mock-Tauri to return a pending multi-question ASK from
  `ask_pending` without first emitting an `ask` event. Require the addressed session to show
  `응답 필요`, answer one tab at a time, revisit and edit a completed tab, then submit mixed
  single/multi/direct input. Require the exact same-session `ask_response` payload plus zero
  horizontal overflow. Emit fenced Mermaid in an archived answer and plan; require real Mermaid
  SVG nodes and no raw `pre/code` fallback.

### Map Agent focused checks

- `cargo test -p isom --test map_agent_native -- --ignored --nocapture` — real StarCraft
  assets plus the rich SCX fixture cover catalog/render/thumbnails, every writable layer,
  semantic ISOM, strict invalid batches, one-save reports, extra MPQ assets, stable locations,
  and absence of a rawgen/new-map path.
- `cargo test -p isom --test map_agent_native terrain_thumbnail_renders_one_exact_tile_and_space_parallax -- --ignored --nocapture`
  — proves exact terrain thumbnails enlarge one tile rather than repeating a tile block and that
  transparent Space Platform tiles reveal the installed star parallax.
- `cargo test -p eud-agent map_model::tests --lib` — preserves the twenty-operation serde
  authority, camelCase names, defaults, and strict unknown-field rejection.
- `cargo test -p eud-agent map_candidate::tests --lib` — covers strict candidate create/open,
  active-draft reopen invariance, startup-only orphan recovery, replay repair, stale-source
  detection, pending-turn commit, and protection.
- `cargo test -p eud-agent map_verify::tests --lib` — covers authority verification and distinct
  missing, empty, and non-parseable candidate draft diagnostics.
- `cargo test -p eud-agent map_stamp --lib` — covers exact live-candidate terrain/object/location
  capture, doodad-overlay deduplication, full-containment collision classification, non-overlapping
  destinations, project-scoped selection-palette rebinding, strict stamp tool schemas, and the
  no-ISOM operation contract.
- `cargo test -p eud-agent exact_selection_stamp_roundtrips_real_map_without_isom --lib -- --ignored --nocapture`
  — with installed StarCraft assets, places a real saved-map selection through the native
  candidate path, requires destination MTXM equality with the source cells, requires no
  `TerrainIsomBrush` in the persisted manifest, and leaves the source SCX byte-identical.
- `cargo test -p eud-agent tools::tests --lib` — requires the advertised `map_draft_patch`
  `inputSchema` to expose exactly all twenty operation alternatives and `map_image_place` to
  expose only request-local `imageRef` plus bounded integer x/y/width/height, all without a
  `parameters` wrapper.
- `cargo test -p eud-agent map_palette --lib` — requires the model-facing palette schema to
  advertise typed filters without pagination and covers blank/browse rejection plus the
  256-result completeness bound.
- `cargo test -p isom --test map_agent_native catalog_structured_filters_narrow_tiles_before_pagination -- --ignored --nocapture`
  and `cargo test -p eud-agent map_palette_query_rejects_catalog_walks_and_returns_complete_filtered_tiles --lib -- --ignored --nocapture`
  — use installed terrain assets to prove native metadata filtering, incompatible kind-field
  rejection, broad tile-walk refusal, and a complete bounded exact-tile result through the agent
  tool path.
- `cargo test -p eud-agent mapsafe::tests --lib` — covers durable pending-Apply crash recovery and
  atomic Apply/undo rails.
- `cargo test -p eud-agent session_kind_lists_keep_eps_and_map_surfaces_isolated --lib` and
  `cargo test -p eud-agent map_tool_list_excludes_original_apply_and_eps_mutations --lib` —
  persisted session isolation and the candidate-only model registry.
- `cargo test -p eud-agent live_saved_open_map_loads_and_renders --lib -- --ignored --nocapture`
  — with the editor bridge live, resolves the exact saved `OpenMapName`, parses it, and requests
  one native crop without writing the map.
- `cargo test -p eud-agent map_image::tests --lib` — covers PNG/JPEG/WebP/GIF-first-frame
  decoding, corrupt input, decode/output caps, aspect resolution, stable tile-grid SHA-256, and
  target/protect/transparent actual-change accounting (the real-asset case is ignored by default).
- `cargo test -p isom --test map_agent_native image_quantizer_uses_only_stable_graphics_valid_tiles_for_every_tileset -- --ignored --nocapture`
  — loads all eight installed tilesets and proves deterministic output, transparent preservation,
  non-empty palette use, and graphics-valid result MTXM.
- `cargo test -p eud-agent no_target_request_mutates_every_supported_candidate_layer --lib -- --ignored --nocapture`,
  `cargo test -p eud-agent direct_image_preview_protect_confirm_and_attachment_free_replay_are_safe --lib -- --ignored --nocapture`, and
  `cargo test -p eud-agent request_local_image_refs_support_multiple_images_and_terrain_patches_in_one_draft --lib -- --ignored --nocapture`
  — prove no-target six-layer mutation, direct preview/confirm/replay safety, request/session ref
  isolation, multiple images, and image/ordinary-terrain ordering with installed assets.
- `cd panel && npx vitest run src/map/imagePlacement.test.ts src/map/ImagePlacementControls.test.tsx src/map/MapAgentApp.test.ts src/map/MapMinimap.test.tsx src/map/MapToolbar.test.tsx src/map/mapProtocol.test.ts`
  — covers centered placement, tile snap, aspect-locked corners/numeric inputs, keyboard deltas,
  report/toggle controls, stale/out-of-order image preview rejection, binary preview envelopes,
  successful draft-mutation generation advances, request-owned draft render IPC, and the explicit
  uncommitted-preview label.
- `cd panel && npx vitest run src/map src/components/Header.test.tsx` — Map button, transforms,
  high-speed/free-mask rasterization and set operations, structured hit-cycle, visible keyboard
  controls, qualifier payloads, and Apply disabled states.
- `cd panel && npx vitest run src/map/MapPalette.test.tsx src/map/StampPlacementControls.test.tsx src/map/SelectionToolbar.test.tsx`
  — requires every saved region to appear as a live palette stamp, exposes direct placement and
  structured stamp mentions, and enforces explicit merge/replace/cancel handling with unsafe
  partial replacement disabled.
- `cd panel && npx vitest run src/map/MapSessionHistoryDialog.test.tsx src/map/mapProtocol.test.ts`
  — Map session history search/load/create/rename/delete controls, active-row protection, and
  map-window-specific IPC payloads.
- Actual Tauri/WebView2 smoke: invoke `map_agent_open` from the main window and require the command
  to resolve, exactly one page at `/map-agent.html`, title `Map Agent Workbench`, a mounted canvas,
  zero alerts, and zero horizontal overflow. Invoke it again and require the same page to focus.
- Mock-Tauri browser smoke returns binary PNG IPC and requires connected bootstrap, exact source
  metadata, original/candidate/diff controls, all six layer controls, a structured saved selection,
  typed mention payloads, AI Elements composer, attachment picker, model/reasoning selectors,
  session context usage, literal `전송` inside the composer, offline retry recovery, and
  `scrollWidth === clientWidth` at 1280×800 and 1920×1080. During a pending Map turn, emit two
  successful draft-mutation tool results and require `수정 중 미리보기 · 미확정`, draft canvas/
  minimap render calls with the exact request id, draft object calls with generation 2, and removal
  of the preview when the turn resolves.
- Actual image-placement Tauri/WebView2 smoke: with a live saved `OpenMapName`, select a PNG through
  `사진 배치`; exercise body drag, corner resize, X/Y/width/height, Arrow and Shift+Arrow,
  original/result toggle, and stale-confirm disable. Require changed/unique/walkability/height/
  protect counts, zero alerts, and zero horizontal overflow at 1280x800 and 1920x1080. Confirm one
  candidate revision, compare original/candidate/diff render and exact bounds, revert r1→r0→r1,
  and require the source SHA-256 unchanged.
- Actual SCX Apply/undo smoke: from that verified candidate, invoke only the trusted toolbar
  `전체 Apply`; require compiling=false, no-share probe success, baseline hash match, a full backup
  whose SHA-256 equals the pre-Apply source, deterministic replay, changed source hash, cleared
  pending journal, and post-write verification. Invoke `마지막 적용 취소` and require exact
  pre-Apply source bytes/SHA-256, then discard the smoke candidate.
- Map session browser smoke: open the history dialog, load an older row, and require its archived
  conversation and session title to replace the visible panel. Create a new row and require the
  previous conversation to clear, with zero horizontal overflow throughout.

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
