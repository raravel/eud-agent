# eud-agent Rules (v2 — Tauri + Rust)

Violations of the EDITOR-* / LUANET-* / MAP-* rules crash or corrupt EUD Editor 3 or
StarCraft at runtime. These are measured behaviors (2026-06-04..07 verification), not
style. v2 retains every crash-critical rule and drops only the rules tied to the removed
in-editor WebView2 hosting and Python server.

## Editor & third-party integrity

- **NEVER modify EUD Editor 3 source, binaries, or repo** (third-party, Buizz). Integration
  = file copies only: `bridge/*.lua` into `Data\Lua\TriggerEditor\`; runtime state under
  `Data\agent\`.
- **The RAG corpus lives in-repo at `ci/corpus/*.jsonl`** (committed, plain git — NOT LFS),
  produced locally by `tools/scraper` from authenticated Naver data and commit-pinned public
  repositories; corpus refresh never runs in CI and secrets are never committed. The legacy
  `chromadb_bge` sqlite (v1, formerly in the ECA repo) is unused and NEVER imported —
  chromadb mutates tracked sqlite on open (proven LFS churn); that caveat is chromadb-specific. The
  distributed RAG index (`rag-index.bin`) is a static read-only CI artifact published to a GitHub
  Release, NOT committed. See [[decisions/15_in-house-rag-corpus]].
- The isom-poc C++ is **vendored** under `native/isom/` and edited only there (our repo is
  source of truth). Add the C ABI shim; keep the verified IsomTerrain/ICU/CascLib code
  paths intact (import-then-extend).
- ALWAYS treat `bridge/ZZZ_10_agent_bridge.lua` as import-then-extend: keep verified v6
  code paths intact; extend, do not rewrite.
- **SCA is fully defunct** — NEVER expose or reintroduce SCA as a settable/creatable type
  (settable text types are CUI/RawText only). ALWAYS force
  `pj.TEData.SCArchive.IsUsed = false` before BUILD.

## Lua bridge (KopiLua/luanet) — crash rules (RETAINED)

The slim v2 bridge keeps only the file-IPC tool layer (PING/STATUS/LIST/GET/SET/NEWEPS/
GETDAT/SETDAT/BUILD/LUA plus additive EPSNAPSHOT) on the `DispatcherTimer.Tick`. WebView2
hosting, panel re-arm, and server spawning are REMOVED.

- NEVER use `os.execute` (KopiLua corrupts it). NEVER use sockets or `io.popen`. Bridge↔app
  IPC is file-based only.
- NEVER rely on lua `pcall` to catch .NET exceptions — they reach the Dispatcher and pop an
  editor error dialog. Isolate risky logic out of Lua.
- ALWAYS access editor objects on the UI thread (`DispatcherTimer.Tick`). NEVER while
  `pgData.IsCompilng` (build shares the lua_State from a BackgroundWorker).
- .NET arrays: ALWAYS use the indexer `arr[i]` (0-based). NEVER `arr:GetValue(i)`.
- VB parameterized properties: ALWAYS `obj:get_PropName(args)` (e.g. `:get_Files(i)`).
- Instance methods use colon; properties use dot. `load_assembly` before `import_type`;
  full assembly names for System/WPF. Enum args: pass enum objects, never raw numbers.
- Empty `StringText` returns nil: ALWAYS `val or ""`.
- NEVER pass a multi-value call (`string.gsub`/`find`) as the LAST arg of another call —
  truncate with parens: `tonumber((string.gsub(...)))` (measured EUD-087).
- Non-ASCII literals in .lua are mojibake (KopiLua reads Latin1): restore with `u8()`.
  Text read via .NET `File.ReadAllText` is fine as-is. **Corollary (v2):** NEVER bake an
  absolute path (data dir, editor path) as a Lua string literal — a non-ASCII Windows
  username corrupts it. The bridge locates `Data\agent\` editor-relative; the app↔editor
  path lives in a config file read via .NET `File.ReadAllText`.
- SET/NEWEPS change memory objects only (user saves). Setter exists only for CUI/RawText;
  GUI files are read-only; LIST must expose the type. NEWEPS duplicate name returns ERROR.
- SETBTN: ALWAYS clear `bs.IsDefault = false` after `PasteFromString` (stale default
  address → wild pointer → StarCraft hard-crash on unit selection; measured 2026-06-07).
- ALWAYS write `heartbeat.txt` AND `status.txt` before the `IsCompilng` early-return in
  Tick (both unconditional). The busy `status.txt` reports `compiling=True` with the
  project line CACHED from the last idle Tick — NEVER touch `pjData` while compiling. In
  v2 these are read by the APP (editor-liveness + build-busy signals), not a self-spawned
  server.
- EPSNAPSHOT MUST enumerate/read every settable text project object regardless of its stored
  filename suffix, plus readable legacy objects whose exact path ends in `.eps`, in one idle
  Tick. It writes request-scoped ordinal files as UTF-8 without BOM and writes `manifest.tsv`
  last. Unreadable files are individual manifest rows. NEVER repurpose the existing DUMP path.
- **DROPPED in v2** (cause removed with in-editor WebView2): panel re-arm via window-handle
  tracking, and the `DispatcherPriority.Normal` mandate against Render-starvation (EUD-039
  — the external panel no longer posts Render work to the editor Dispatcher). The default
  timer is acceptable; the unconditional heartbeat/status writes above still stand.

## IPC and encoding (RETAINED)

- ALWAYS write IPC files (`.cmd`/`.result`/config/heartbeat/status and EPSNAPSHOT
  manifest/content) as UTF-8 **without BOM** (Rust: write bytes; Lua snapshot:
  `UTF8Encoding(false)` — a BOM breaks first-line parsing).
- The app deletes each `.result` after consuming and clears stale inbox/outbox at startup.
  The bridge deletes `.cmd` after processing.
- App command files are named `srv-<uuid8>.cmd`; a consumer polls only its own basenames.
- NEVER poll `.result` without a timeout. Default 10s; use 180s for full-project
  `EPSNAPSHOT` scans, and extend other requests to 180s when `status.txt` says
  `compiling=true` while emitting `progress {stage: waiting_build}` to the panel.

## Agent epScript project architecture

- `GETMAIN` over `pj.TEData.MainFile` is the ONLY MainFile authority. `project_status.mainFile`
  exposes its exact project-relative `/` path or `null`; `list_files` remains authoritative for
  path/type/settable metadata. NEVER infer MainFile from a filename, list order, open tab,
  lifecycle hooks, file count, or project memory.
- Preserve a configured MainFile as the composition root regardless of name. NEVER rename it or
  call `set_main` merely to normalize naming, and NEVER treat another file named `main` as special.
  When `mainFile` is null, a non-empty project requires an explicit reviewed selection; localized
  work that does not need a start-file change proceeds without setting one.
- Put behavior in the existing module that owns the mutable state and invariant. Create a module
  only for a distinct cohesive responsibility with a narrow API; extract a stable leaf only after
  two real consumers exist.
- Keep imports directional and acyclic:
  `configured MainFile -> feature modules -> stable leaf modules`. Candidate-introduced
  `EUDLSP002` cycles MUST be removed; report unrelated pre-existing cycles without refactoring them.
- NEVER create empty scaffolding or generic `utils`, `common`, `helpers`, or `state` dumping
  grounds. More than 800 nonblank lines is only a cohesion review signal, NEVER an automatic
  split or build gate.
- Localized fixes stay in the owning file. Broad splitting, moving, renaming, or unrelated code
  cleanup requires reviewed scope.
- After file topology, MainFile, direct dependency, or material responsibility changes, replace
  project memory `structure` completely with every file's current role and direct imports.
  Ordinary internal edits that leave roles and dependencies unchanged MUST NOT churn memory.
- Preflight every mutually dependent candidate in one `eps_check` batch, remove newly introduced
  cycles, apply the coherent changeset, and run the mandatory complete-project `build_run`.

## Agent epScript preflight process

- `eps_check` is read-only and advisory: NEVER journal it, consume mutation/action budget,
  gate a write, change evidence/plan/build state, or emit panel diagnostics state.
- Candidate paths MUST be normalized project-relative `.eps` paths, collision-checked
  case-insensitively, and confined beneath `%localappdata%\eud-agent\lsp_workspaces`. Each
  candidate contains exactly one of complete `code` or ordered exact `edits`; edit candidates
  resolve against the reusable mirror before analysis. Candidate batches overlay atomically in a
  disposable analysis directory; never copy candidate data back to editor state.
- The adapter resource, checksum, MIT license, and provenance are bundled from the pinned
  upstream commit. Verify SHA-256 before lazy startup. NEVER install npm packages,
  download code, invoke a shell, execute project code, or bundle a Node runtime at runtime.
- Spawn resolved `node.exe` directly with piped stdio, no shell/window, bounded stderr,
  Content-Length framed JSON, finite cold/warm deadlines, and process-tree reaping.
  Calls are serialized. Retry a crash/timeout/protocol failure at most once per check and
  suppress repeated starts for the rest of that request.
- Missing Node/resource, snapshot failure, startup failure, crash, timeout, or malformed
  protocol MUST return the stable successful `skipped` result. All other tools remain usable.
- `EUDLSP001` missing import is an error; `EUDLSP002` cycle is an advisory warning;
  `EUDLSP003` is a case-insensitive path collision; `EUDLSP004` is an imported unreadable
  snapshot file. Final correctness always comes from the existing mandatory `build_run`.

## codex invocation (Rust, Windows) (PORTED)

- NEVER spawn bare `"codex"`. ALWAYS resolve the app-managed executable first and
  fall back to the `which` crate (fail fast if unresolved).
- App-server launch cwd is app-owned, never the repository or process launch dir. Project
  turns use `%appdata%\eud-agent\workspaces\.sessions\<project-id>\<session-id>`; the
  canonical accepted workspace is never a writable Codex cwd.
- Read turns MUST use `eud_workspace_read`: `:minimal` runtime reads, current session root
  read-only, network disabled, elevated exact-root Windows backend.
- Write turns MUST use `eud_workspace_write`: `:minimal` runtime reads, only the current
  session root writable, `source/**` read-only, network disabled, elevated exact-root backend.
  Write mode is available only to the exact project/session/request lease owner. Unsupported
  setup fails closed; NEVER downgrade to legacy Windows workspace-write.
- App-server command-execution requests MUST be auto-approved in both read and write modes so
  native commands and Code Mode JavaScript run inside the active named profile. File-change,
  patch, and generic permission-expansion requests MUST remain denied; command approval MUST
  NEVER widen the profile's filesystem or network boundary.
- Trusted baselines and acceptance metadata stay in `workspaces/.state/`, outside every Codex
  cwd. Each session `source/` is replaced from one coherent EPSNAPSHOT and is NEVER a
  live-editor write path.
- Native filesystem changes MUST be scanned and journaled at write-turn end, including timeout
  cancellation. Accept promotes selected session bytes to canonical storage under the lease;
  promotion and metadata failure MUST restore canonical bytes. Reject restores only the session
  root.
- `plan_approve` MUST acquire the project write lease before atomically writing the exact
  approved Markdown to canonical `plans/<request-id>.md`. The plan is synced before the
  execution baseline and survives implementation rejection. Codex MUST NEVER edit, rename, or
  delete that authoritative file.
- An approved-plan execution MUST NOT report normal completion until the backend verifies
  all project-wiki postconditions: the exact plan snapshot; non-empty `specs/index.md`
  linking a non-empty `specs/*.md` topic page; and `worklog/<request-id>.md` recording the
  actual result/verification and linking a canonical topic spec. Specs describe implemented
  reality, NEVER merely intended work. Missing artifacts get at most two focused repair
  turns, then a visible error; they are never silently waived.
- Manual conversation compaction MUST use app-server `thread/compact/start` for the exact saved
  thread and MUST wait for the completed `contextCompaction` item. NEVER synthesize a panel-side
  summary, send `/compact` as user text, clear panel history, or alter backend plan/review state.
- The model-specific 1M opt-in MUST inject `model_context_window=1000000` and
  `model_auto_compact_token_limit=900000` through every thread start/resume while enabled, and
  omit both on the next resume when disabled. Drivers MUST reload this app setting before every
  turn. NEVER hard-code a capability allowlist or override Codex's model catalog: an effective
  context below 900,000 is a safe default-window fallback and gets one visible warning per
  session/model.
- Different sessions MAY overlap read-only Codex turns; commands within one session MUST stay
  serialized. Conversation submission MUST NOT be globally queued.
- Unexpected app-server exit or stdio closure MUST interrupt the current turn without replaying
  it, retain the thread id, capture bounded stderr/exit diagnostics, and respawn before the next
  command. A dead client MUST NEVER be reused.
- Project mutations MUST register FIFO write intent and retain one lease through mutation,
  build, review, and complete decision work. Partial decisions, rollback failure, undecided
  journals, or unpromoted workspace changes MUST NOT release it.
- On restart, one valid pending journal MUST restore its review lease before any new writer.
  Conflicting pending writers are an explicit error. Session selection MUST NOT change backend
  ownership.
- Every conversation event and cancellation target MUST carry the immutable session id. A
  mutable global session fallback is forbidden.
- App-server permission/file/command escalation requests remain declined; only the
  `eud-tools` MCP elicitation is accepted. Live editor/map/DAT/build operations MUST still
  use eud-tools.

## Map file writes (mapsafe + isom FFI) (RETAINED)

- NEVER save a map with location auto-defragmentation: ALWAYS `autoDefragmentLocations=
  false` and `lockAnywhere=true` (defrag RENUMBERS MRGN slots and silently re-points every
  trigger's location reference). Location ids stay stable; "delete" = zero the slot in
  place.
- NEVER edit location #64 (Anywhere) — protected at the C ABI.
- ALWAYS take a full-file backup BEFORE any map write (`%appdata%\eud-agent\map_backups`,
  timestamped) — the journal's rollback source (temp + atomic replace).
- ALWAYS refuse the write while the map file is open elsewhere (CreateFileW no-share probe
  → sharing violation = SCMDraft has it) or while STATUS reports `compiling=true`.
- Location NAME bytes follow the map's OWN string-table encoding — pass them through the C
  ABI as **raw bytes**; NEVER re-encode in Rust or C++.
- locedit/playeredit/switchedit apply all-or-nothing: any invalid op aborts BEFORE
  save. Verify by re-digesting the map after every write.
- player_setup edits start-location units (214) + OWNR controllers through the SAME rails;
  its save also keeps `autoDefragmentLocations=false`.
- switch_write edits SWNM/string data only. Switch ids are 1-256 and trigger
  conditions/actions keep their numeric ids unchanged. Switch NAME bytes follow
  the map string-table encoding and pass raw through the C ABI.

## Map Agent candidate authority

- The Map Agent MUST resolve only the current saved `OpenMapName`. It MUST display the saved
  source path, mtime, and file hash; unsaved editor/SCMDraft state is not authority.
- Model-visible Map tools MUST mutate only the active request draft. They MUST NOT expose original
  Apply, backup restore, arbitrary filesystem paths, or EPS mutation tools.
- `MapMentionSnapshot` is the only structured prompt constraint. NEVER parse chip labels or
  natural-language text into selection, object, location, palette, owner, count, or state
  authority.
- When the current Map request has no target mention, the entire current candidate is writable
  for the supported terrain, units, buildings, doodads, sprites, and locations layers. A missing
  target MUST NOT cause mutation refusal or an area-selection question.
- One or more current-request target mentions narrow coordinate writes to the union of their exact
  canonical cells and explicit layers. A stored target omitted from the current request does not
  constrain it. Reference and anchor mentions are read/comparison context and MUST NOT expand or
  narrow write scope. Protect masks always remove their cells/layers, including persistent
  protections omitted from later prompts; an empty protect layer set means all supported layers.
- `MapRequestAuthority::calculate` is the only scope calculation. Candidate patch, selection-stamp
  placement, image conversion, per-batch verifier, finalize, deterministic replay, and Apply
  verification MUST use its persisted result. Native transitions MUST NOT be clipped or hidden to
  evade it.
- Object and location mentions remain bound to the exact candidate revision key and baseline file
  hash. Object fingerprints plus candidate-local UUIDs MUST resolve exactly; stale, missing, or
  ambiguous instances fail closed. Palette mentions are current-tileset type/style references,
  never placement authority.
- Every saved selection MUST have exactly one project-scoped palette entry for the saved map.
  Selection create/update MUST atomically upsert the canonical mask/label/role/layers; selection
  delete MUST remove the entry. Each Map session MUST bind that shared definition to its own
  visible candidate revision. A stamp mention identifies the selection but NEVER grants placement
  authority.
- Region stamps MUST read source content from the visible candidate at placement time and resolve
  server-side to exact typed operations. The model and React MUST NOT read, enumerate, infer, or
  provide the source MTXM matrix. Exact copy/duplicate requests MUST NOT be reconstructed through
  render comparison, catalog walks, expected-before probes, semantic ISOM, or approximate flat
  tiles.
- A stamp copies only its canonical mask and selected supported layers; an empty layer set means
  all six supported layers. Units, buildings, doodads, sprites, and locations MUST be included
  only when their complete footprint lies inside the source mask. Destination top-left positions
  MUST be bounded and non-overlapping. Terrain writes exact MTXM/TILE values and NEVER receives
  ISOM boundary correction.
- Stamp preview MUST be read-only. Terrain replacement is expected and is not an object collision.
  Existing destination objects or locations require an explicit user merge, replace, or cancel
  choice. Merge preserves destination objects and adds copies. Replace may delete only
  fully-contained selected-layer items; a boundary-crossing item fails closed. Both paths are
  all-or-nothing and MUST pass current target/protect authority, free-location-slot, native edit,
  per-batch verification, finalize, and deterministic replay rails.
- Image attachments create no write permission, image mention, whole-map authority, or permission
  toggle. Current-request images are opaque `image-N` bindings. `map_image_place` accepts only that
  ref and integer tile placement, server-generates one normal `TerrainBlit`, and passes it through
  the same terrain authority and verifier as `map_draft_patch`. It MUST NOT expose attachment ids,
  local paths, palettes, MTXM ids, tile matrices, original Apply, or unsupported CHK surfaces.
- Image decode MUST retain the 10 MiB encoded limit, reject zero/oversized dimensions before
  allocation, cap decode at 16,777,216 pixels, cap output at 256x256 tiles, and accept only PNG,
  JPEG, WebP, or the first GIF frame. The native palette MUST contain only graphics-valid tiles
  from the current tileset and keep deterministic scan-order dedupe/ties.
- Fully transparent output cells keep candidate MTXM and consume no target/protect scope. Partial
  alpha MUST composite against the candidate tile representative color before quantization.
  Preview never mutates candidate/source state; confirm recomputes and checks revision, baseline,
  attachment SHA-256, transform, digest, map bounds, authority, and protect.
- Every successful draft batch is verified immediately and again at finalize/replay. Stamp and
  image batches do not seal a draft; multiple stamps, images, and normal terrain patches may be
  interleaved. Native
  finalize remains request-local until the complete model turn succeeds, then publishes at most
  one visible revision. Failure, cancellation, stale preview, or abandoned drafts MUST leave the
  visible candidate and source byte-for-byte unchanged.
- Live Map workbench preview MAY read only a verified active request draft after a successful
  scoped draft-mutation tool result. It MUST bind render and object requests to the exact session,
  parent candidate revision, request id, and monotonically increasing panel generation; stale
  output MUST be discarded. The UI MUST label the draft uncommitted, MUST NOT create candidate
  object mentions from it, and MUST clear it on success, failure, or cancellation. Preview state
  MUST NOT publish a candidate revision, authorize writes, or change source bytes.
- Original Apply and undo are user-only commands accepted from the `map-agent` window label.
  Apply is whole-candidate and atomic; it MUST pass coordinator serialization, compiling, lock,
  source-hash, backup, deterministic replay, and post-write verification rails. Partial layer
  Apply and model-triggered Apply are forbidden.
- Apply MUST persist and flush a pending transaction record before replacing the source. Startup
  restores an uncommitted interrupted Apply; a committed candidate state prevents false rollback.
  The record is cleared only after candidate state persistence or exact undo.

## Rust / C++ FFI (NEW)

- The C↔Rust boundary is plain C ABI only: `extern "C"`, no C++ STL types or exceptions
  across it. Pass paths + op buffers in, return status codes + out-params; free
  C-allocated buffers with the matching `isom_free`. A C++ exception must be caught at the
  shim and converted to an error code — NEVER allowed to unwind into Rust.
- The engine is **statically linked** (Decision 09) — no `.dll` shipped or loaded. The
  static `.lib` is produced by MSBuild; `isom-sys/build.rs` emits the link directives and
  bindgen generates the header bindings. Build requires the MSVC toolchain (same as Rust
  MSVC target).
- Map-write safety rails (backup, lock probe, compiling guard, journal/rollback) live in
  the Rust `mapsafe` layer, NEVER in C++ — keep the C ABI to pure byte-level map ops.

## Tauri app, panel, data dirs (NEW + PORTED)

- Panel ↔ core is **Tauri IPC** (`invoke` + events) only — NO localhost socket, token, or
  Origin check, and NO `server.ready` (Decision 11). Reasoning renders dim/collapsible,
  answers prominent; NEVER render raw `agent_event` kind identifiers as user-facing text.
- `ask` MUST be non-mutating and session-scoped. It may wait only on its per-session response
  channel, NEVER while holding the session engine mutex. Every question id is unique; every
  question must be answered; single-choice cardinality and multi-choice bounds are validated
  backend-side. Cancel and a dropped MCP ASK future MUST release the pending tool call. The panel
  MUST reconcile the backend-authoritative pending request through `ask_pending` after registering
  event listeners; a successful Tauri event emit alone is NEVER proof that the ASK was displayed.
- Mermaid is enabled only for AI answers (live and archived) and plan cards. It MUST use the
  bundled `@streamdown/mermaid` package; NEVER fetch scripts, styles, or renderers from a CDN.
- NEVER load panel assets from a CDN — JS/CSS/fonts/Monaco workers/Streamdown assets are
  bundled. Monaco MUST load from the `monaco-editor` npm bundle via `loader.config({
  monaco })`; the `@monaco-editor/react` default CDN loader is forbidden.
- Monaco is the edit surface; the diff tab renders the Rust-supplied unified diff with +/-
  coloring (NEVER Monaco DiffEditor — the core does not ship original file content).
- Data dirs (Decision 12): IPC under the editor's `Data\agent\`; app user data under
  `%appdata%\eud-agent\`, including durable per-project `workspaces/`; large/regenerable
  assets and analyzer mirrors (model, RAG index, logs, `lsp_workspaces`) under
  `%localappdata%\eud-agent\` (NEVER Roaming).
- Bootstrap: every downloaded asset is **sha256-verified** and placed **atomically** (temp
  + rename). A missing/corrupt asset re-downloads; it must never half-install.
- WebView2 uses the system Evergreen runtime; if absent, guide the user to install it
  (do not silently fail).
- RAG model loading must NEVER gate app readiness (lazy load + background warmup; report
  `rag_warmup` progress). The panel is usable before the model finishes loading.

## Release & self-update (NEW — Decision 17)

- The updater uses **minisign** signing (`createUpdaterArtifacts: true`, bundle target
  `["nsis"]`). The minisign **private key is NEVER committed** — keep it under
  `%USERPROFILE%\.tauri\` and inject via `TAURI_SIGNING_PRIVATE_KEY` /
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` at build time. Only the **public** key lives in
  `tauri.conf.json` (`plugins.updater.pubkey`). This minisign signing is SEPARATE from
  Windows Authenticode (intentionally absent; SmartScreen warning is an accepted trade-off).
- A `v*` tag runs `.github/workflows/publish-app.yml` on Windows. The workflow MUST verify
  the tag against `tauri.conf.json`, `src-tauri/Cargo.toml`, and `Cargo.lock`, then use
  `tauri-action` to build, minisign-sign, publish the NSIS installer, and generate/upload
  `latest.json`. Repository secrets hold the private key and password.
- A local `tauri build` does NOT emit `latest.json`. The fallback release script
  (`scripts/release.ps1`) MUST synthesize it from the `.sig` and upload it. The updater endpoint
  is the static `releases/latest/download/latest.json`, so the app release must remain GitHub's
  latest release. Write locally generated `latest.json` and bumped manifests as UTF-8 without BOM.
- The self-update replaces ONLY the app binary. `%localappdata%` assets (model/RAG) and
  `%appdata%` config are preserved and NEVER re-downloaded by an update — RAG/model asset
  *versioning* stays the `bootstrap` manifest's job, decoupled from the app updater.
- The bridge Lua is bundled as a Tauri **resource** and re-installed on EVERY app start via
  `bridge_install::sync_bridge` (Rust port of `install_bridge.ps1`): overwrite the editor's
  `Data\Lua\TriggerEditor\ZZZ_10_agent_bridge.lua` only when bytes differ, copied **verbatim**
  (KopiLua reads Latin1 — never re-encode). It is best-effort: a downed/moved editor NEVER
  blocks startup (log + continue). Editor integrity rule still holds — this is a file copy,
  not an editor modification.
- The update check runs ONCE per session, only after first-run setup is satisfied, and NEVER
  gates the panel — offline / no-release / a check error simply shows no banner. The banner
  requires user consent before downloading; raw updater errors render in the banner, never as
  silent failures.
- The NSIS installer is `installMode: currentUser` — KEEP it: a perMachine installer would
  make the updater's self-replace require UAC elevation on every update. Installer branding
  (header 150x57 / sidebar 164x314, 24-bit BMP) is regenerated from the app icon by
  `scripts/gen_installer_assets.ps1`; a Desktop shortcut is added/removed via the
  `NSIS_HOOK_POSTINSTALL`/`POSTUNINSTALL` macros in `src-tauri/installer/hooks.nsh`.
  `languages: [Korean, English]` with `displayLanguageSelector` (Korean is the primary
  distribution target).

## System prompt, evidence, first principles (PORTED verbatim intent)

- The system prompt ALWAYS carries the `[first principles]` section (known crash/EUD-error/
  drop/freeze causes + `## eps idioms`, source: cafe edac/91492) BEFORE the `[reference
  context]` RAG section — never-do rules outrank retrieved examples. Requests that would
  violate one are REFUSED with the item number and a safe alternative.
- The tool layer mechanically backs the principles: `btn_set` REJECTS any disableable
  button (`actval != 0`) whose `disstr` (field 8) is 0; `xdat_set` REJECTS reassigning a
  unit's `ButtonSet` to a different set id — always edit the unit's OWN set in place.
- Evidence gate (EUD-090): mutating tool calls are REJECTED (`EvidenceRequired`) until one
  `search_docs` has RUN in the request (RAG-wired layers only); zero hits still lift the
  gate (mark items 근거 없음 (일반 EUD 지식) — NEVER fabricate a source). Exempt:
  `memory_write`, `build_run`. The `[evidence]` section requires why + a source link
  (`[제목](url)`) on propose_plan steps AND the final answer; `[reference context]` chunks
  carry `source:` headers. Crash diagnosis MUST first match `[first principles]` with the
  item number cited (or "no item matches") before any fix.
- Resumed turn text ALWAYS labels the user's text with a `[user message]` header after the
  prepended context; the system prompt carries the `[message format]` section (only
  `[user message]` is the instruction; a bug report there is a work request) (EUD-092).
- The `[eps preflight]` section precedes `[build]`: submit every candidate in one batch using
  complete code for creates/full rewrites or the same ordered exact edits used by `file_edit`, fix
  error diagnostics, and re-check before `.eps` writes; mutually dependent files travel together.
  A skipped check falls through to writes and mandatory `build_run`. epscript-lsp diagnostics are
  advisory only — annotate, never block apply; absence must not break the flow. There is
  intentionally no mechanical “eps_check required” gate.

## Process

- All spec/task content in English; user-facing conversation in Korean.
- Single editor instance per machine is the supported topology (documented limitation).
- Windows E2E steps needing the editor GUI are user-assisted; everything else verifiable
  headless via verify.md.

## Learned rules

- [LEARNED 2026-06-10 from EUD-144-013d] NEVER change `DEFAULT_BATCH_SIZE` (16) in `ci/build_rag_index.rs` and NEVER pass `--batch`/`BATCH_SIZE` in the CI index build: BGEM3Q int8 embeddings shift ~2% with batch size (measured EUD-144, cosine ~0.98), so any other batch breaks byte-equivalence with the published rag-index v1 and the EUD-107-verified embedding space. `--batch` exists only for local throughput experiments whose output is discarded. ORT intra-op threads are output-neutral (safe).
