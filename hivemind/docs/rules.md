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
  produced locally by the Node/TS Naver-Cafe scraper (`tools/scraper`, cookie-gated, never in CI).
  The legacy `chromadb_bge` sqlite (v1, formerly in the ECA repo) is unused and NEVER imported —
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
GETDAT/SETDAT/BUILD/LUA) on the `DispatcherTimer.Tick`. WebView2 hosting, panel re-arm,
and server spawning are REMOVED.

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
- **DROPPED in v2** (cause removed with in-editor WebView2): panel re-arm via window-handle
  tracking, and the `DispatcherPriority.Normal` mandate against Render-starvation (EUD-039
  — the external panel no longer posts Render work to the editor Dispatcher). The default
  timer is acceptable; the unconditional heartbeat/status writes above still stand.

## IPC and encoding (RETAINED)

- ALWAYS write IPC files (`.cmd`/`.result`/config/heartbeat/status) as UTF-8 **without
  BOM** (Rust: write bytes / `encoding=utf-8` equivalent; never a BOM — first-line command
  parsing breaks). `File.ReadAllText` strips an incoming BOM, so bridge reads are safe.
- The app deletes each `.result` after consuming and clears stale inbox/outbox at startup.
  The bridge deletes `.cmd` after processing.
- App command files are named `srv-<uuid8>.cmd`; a consumer polls only its own basenames.
- NEVER poll `.result` without a timeout. Default 10s; extend to 180s when `status.txt`
  says `compiling=true` and emit `progress {stage: waiting_build}` to the panel.

## codex invocation (Rust, Windows) (PORTED)

- NEVER spawn bare `"codex"`. ALWAYS resolve via the `which` crate to the `.cmd` shim path
  (fail fast if unresolved). A `CODEX_CMD` env/config override may supply a full path.
- ALWAYS pass `--skip-git-repo-check`; set cwd to a stable working dir.
- ALWAYS pass the prompt via **stdin** (write then close). NEVER as argv (32,767-char
  CreateProcess limit; RAG context exceeds it). ALWAYS give every subprocess an explicit
  stdin (the prompt pipe). Use `tauri-plugin-shell` / tokio with piped stdio.
- Treat codex stdout as noisy: extract fenced code blocks; if none, fail with the raw
  output in the error rather than applying noise to the editor.

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
- locedit/playeredit apply all-or-nothing: any invalid op aborts BEFORE save. Verify by
  re-digesting the map after the write.
- player_setup edits start-location units (214) + OWNR controllers through the SAME rails;
  its save also keeps `autoDefragmentLocations=false`.

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
- NEVER load panel assets from a CDN — JS/CSS/fonts/Monaco workers/Streamdown assets are
  bundled. Monaco MUST load from the `monaco-editor` npm bundle via `loader.config({
  monaco })`; the `@monaco-editor/react` default CDN loader is forbidden.
- Monaco is the edit surface; the diff tab renders the Rust-supplied unified diff with +/-
  coloring (NEVER Monaco DiffEditor — the core does not ship original file content).
- Data dirs (Decision 12): IPC under the editor's `Data\agent\`; app user data under
  `%appdata%\eud-agent\`; large/regenerable assets (model, RAG index, logs) under
  `%localappdata%\eud-agent\` (NEVER put the 570MB model in Roaming).
- Bootstrap: every downloaded asset is **sha256-verified** and placed **atomically** (temp
  + rename). A missing/corrupt asset re-downloads; it must never half-install.
- WebView2 uses the system Evergreen runtime; if absent, guide the user to install it
  (do not silently fail).
- RAG model loading must NEVER gate app readiness (lazy load + background warmup; report
  `rag_warmup` progress). The panel is usable before the model finishes loading.

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
- epscript-lsp diagnostics are advisory only — annotate, never block apply; absence must
  not break the flow.

## Process

- All spec/task content in English; user-facing conversation in Korean.
- Single editor instance per machine is the supported topology (documented limitation).
- Windows E2E steps needing the editor GUI are user-assisted; everything else verifiable
  headless via verify.md.
