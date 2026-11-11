# Tech stack

## Desktop shell

- Windows 10/11, x64, MSVC Rust target
- Tauri 2
- System WebView2 runtime
- Tauri plugins: dialog, shell, updater, process, single-instance
- No localhost web server, Python application server, Electron, or Editor-hosted UI

## Backend

- Rust workspace; `src-tauri` application crate
- Tokio/Tauri async runtime for commands and event delivery
- `serde`/`serde_json` for typed IPC and canonical project files
- `sha2` for revisions, fixture identity, and asset verification
- `parking_lot`/standard synchronization for session services and project write coordination
- `reqwest` for managed release/bootstrap downloads
- `fastembed` for in-process BGE-M3 retrieval

Native domain modules:

- `native_project`: manifest, EPS tree, sparse DAT, atomic persistence
- `native_runtime`: configured project facade and project-scoped build marker
- `native_build`: DAT catalog, deterministic generators, euddraft runner/diagnostics

### E2 compatibility stack

The E2 implementation is a bounded Rust pipeline in `src-tauri/src/e2_import/`: `parser.rs` reads UTF-8/CP949-compatible `.e2s` and text `.e2p` input; `definitions.rs` and `compiler/` hold the independent 0.17.9.7 action/condition inventory and editable EPS projection; `dat_projection.rs` maps width/sign/VarStart-aware deltas and FireGraft/TBL/requirements/buttons; `compat/` emits BIN, both tile modes, plugin, and file-manager compatibility sources; `bgm/` creates BGM state and ordered assets; `resources/` resolves and hashes read-only source bytes; and `transaction/` stages, publishes, reopens, rolls back, and activates.

The source boundary is the verified armoha v0.17.9.7 tag at `2baf971ee493c772c609a938e0daffba3c40792c` and release archive SHA-256 `7e986a6511749f088d485e67c5c3d17817e5de953bce8aa363ee7c28323fc66a`. The historical `Buizz/EUDEditor` commit `8582ec5f240d1482f69ef3f8fadd400e6fe2de86` is comparison-only. Runtime code contains clean-room typed facts and the narrow attributed seven-function helper; it does not load `.research`, install Editor2, consume an Editor2 archive, bundle game binaries, or vendor archive-only metadata with an unsupported license inference.

`ProjectManifest.e2_import` is optional under schema v2. `E2ImportManifest` records version, source kind, preflight fingerprint, owned `ProjectResource` entries, and disabled-section provenance. `NativeProject::validate_resources`, `resource_snapshot`, and `revision` enforce normalized paths, exact bytes, and deterministic ordering. The resource consumer enum is explicit: `SourceMap`, `Mpq`, `BinCompat`, `TileCompat`, `Plugin`, `Dat`, and `FileManager`. Native build resolves only those confined entries, generates `src/e2/{bin_compat,tile_compat,bgm}.eps` and file-manager/plugin consumers, and rejects persisted generated resources.

The build path uses the existing euddraft 0.10.2.5 runner and eudplib 0.80.6 contract. BGM segments are emitted by generated `E2MpqAssets.py` as `with open(..., "rb")` followed by `MPQAddFile("M<n>_<k>.ogg", asset.read(), False)`, before freeze. `verify_e2_output_assets` reads the fresh map's MPQ entries through the bounded native reader and compares bytes and SHA-256. `PythonPath.py` only supplies helper import search paths; generated helper code is not a Python entrypoint.

The panel transport uses closed discriminated unions in `panel/src/lib/projectImport.ts` and `protocol.ts`, with event name `e2_import`. `E2ImportDialog.tsx` subscribes before invoking import and guards events by attempt, import id, and fingerprint. Native registration is in `src-tauri/src/lib.rs`; command implementations and picker filters are in `src-tauri/src/setup.rs`. The three host mounts are `App.tsx` launcher, `SetupScreen.tsx`, and `SettingsDialog.tsx`. Stable QA labels are `E2 프로젝트 가져오기`, `E2S/E2P 선택`, `원본 맵 다시 연결`, and `가져오기 취소`.
- `nrbf` + `e3s_nrbf`: independent E3S graph compatibility
- `tool_exec`/`tools`: schema-rich MCP runtime
- `journal`/`workspace`/`memory`: durable review state; accepted harness documents/memory live with the project, while journals and session working state stay machine-local
- `harness_import`: scoped omission/consent identities and no-replace, same-volume import publication
- `isom`/`mapsafe`/`chk`: map reads and safe mutations

## Native project format

- `project.eap`: schema version 2 JSON manifest (EUD Agent Project); a renamed sole `.eap` is supported
- legacy `project.json`/`.eudproj`: explicit one-time migration inputs only
- sparse JSON DAT documents: schema version 1
- EPS and direct eudplib Python sources under `src/`
- SCX/SCM maps under `maps/` and generated outputs under `build/`
- atomic temp-write/replace; UTF-8 without BOM

SQLite is not an authoring authority. EDS, generated Python, requirement binaries, custom TBL, and E3S are outputs.

## Build toolchain

- Latest official `armoha/euddraft` release ZIP installed by default with SHA-256 verification, or a configured existing euddraft folder/executable; direct-Python projects require its frozen `euddraft.exe`
- Deterministic EDS/plugin generation in Rust
- Bundled EUD Editor open-source compatibility data (`.def/.dat`, offsets, TBLs, wireframe helper) under its original license
- Sibling `../euddraft` is source-read-only; product code never patches it
- EUD Editor executable/assemblies are neither bundled nor loaded at runtime

## Panel

- React 19 + TypeScript
- Vite
- Tailwind CSS 4
- shadcn/ui/Radix primitives
- Lucide icons
- Zustand-style local store implemented in project code
- Vitest + Testing Library + jsdom
- No runtime CDN

## Agent providers and tools

- Closed provider set: Codex, Claude Code, Antigravity, OpenCode Go, OpenCode Zen, and OpenAI 호환 (internal id `ollama`: any OpenAI-compatible Chat Completions endpoint, e.g. Ollama or EXAONE)
- Typed provider/model/reasoning bindings are immutable per saved session
- Codex and other CLI providers use their supported native authentication/runtime paths; Ollama uses its configured local/base URL
- Typed session protocol over Tauri invoke/listen
- MCP tool registry generated from Rust schemas
- One batch `dat_patch` mutation API; individual model-facing DAT setters removed
- RAG index/model managed under LocalAppData with sha256 verification

## Map and media

- Statically linked `isom` C ABI for CHK extraction/mutation
- Windows share-mode lock probe
- Managed checksum-pinned FFmpeg/FFprobe for audio only
- PNG/image processing libraries already pinned by the workspace

## Compatibility

- MS-NRBF implemented in Rust without `BinaryFormatter` execution
- Product acceptance uses the original .NET BinaryFormatter only as an external verifier
- Supported E3S projection: CUI EPS, MainFile, settings/plugins, standard DAT, XDAT, TBL, requirements, buttons
- Unsupported GUI/RawText or Classic-as-main inputs fail closed

## Packaging

- Tauri resource `native/eud-editor-compat`
- panel production output in `panel/dist`
- Windows `.eap` association and dedicated `icons/project.ico`, owned solely by NSIS hooks; queued startup/second-instance file-open delivery
- signed updater artifacts per the existing release decision
- no Lua bridge, install script, Editor path, Editor DLL, inbox/outbox, or heartbeat resource

## Python build dependencies

- Pinned managed `uv` 0.11.3 Windows x64 ZIP with embedded SHA-256.
- Exact PyPI wheels only; no source distributions, ranges, VCS/path sources, ambient pip/Python/uv, or alternate indexes.
- Deterministic lock records normalized name, exact version, wheel filename/tags, HTTPS PyPI URL, SHA-256, frozen Python ABI, euddraft fingerprint, and uv version.
- Verified immutable downloads/environments are derived under `%LOCALAPPDATA%/eud-agent` and may be reused offline.
