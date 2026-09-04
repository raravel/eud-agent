# Tech stack

## Desktop shell

- Windows 10/11, x64, MSVC Rust target
- Tauri 2
- System WebView2 runtime
- Tauri plugins: dialog, shell, updater, process
- No localhost web server, Python application server, Electron, or Editor-hosted UI

## Backend

- Rust workspace; `src-tauri` application crate
- Tokio/Tauri async runtime for commands and event delivery
- `serde`/`serde_json` for typed IPC and canonical project files
- `sha2` for revisions, fixture identity, and asset verification
- `parking_lot`/standard synchronization for session services and project write coordination
- `reqwest` for managed release/bootstrap downloads
- `fastembed` for in-process BGE-M3 retrieval
- framed Node adapter only for pinned epScript analyzer integration

Native domain modules:

- `native_project`: manifest, EPS tree, sparse DAT, atomic persistence
- `native_runtime`: configured project facade and project-scoped build marker
- `native_build`: DAT catalog, deterministic generators, euddraft runner/diagnostics
- `nrbf` + `e3s_nrbf`: independent E3S graph compatibility
- `tool_exec`/`tools`: schema-rich MCP runtime
- `journal`/`workspace`/`memory`: durable review state
- `isom`/`mapsafe`/`chk`: map reads and safe mutations
- `eps_preflight`: full native source snapshots and analyzer gate

## Native project format

- `project.json`: schema version 1
- sparse JSON DAT documents: schema version 1
- EPS sources under `src/`
- SCX/SCM maps under `maps/` and generated outputs under `build/`
- atomic temp-write/replace; UTF-8 without BOM

SQLite is not an authoring authority. EDS, generated Python, requirement binaries, custom TBL, and E3S are outputs.

## Build toolchain

- Configured euddraft 0.10.x executable or source entrypoint
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

- Closed provider set: Codex, Claude Code, Antigravity, OpenCode Go, and Ollama
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
- signed updater artifacts per the existing release decision
- no Lua bridge, install script, Editor path, Editor DLL, inbox/outbox, or heartbeat resource
