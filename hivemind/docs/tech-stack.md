# eud-agent Tech Stack (v2 — Tauri + Rust)

The v2 runtime removes Python, keeps the React panel, and runs the backend in Rust/Tauri.
The only Node runtime path is the optional agent-only epScript analyzer, which uses the
user/system `node.exe`; no Node runtime is bundled.

## Active Dependencies (panel — kept, from `panel/package.json`)
- react 19.2.0 — panel UI
- react-dom 19.2.0 — DOM renderer
- @monaco-editor/react ^4.7.0 — Monaco React wrapper (CDN loader forbidden; bundled)
- monaco-editor ^0.55.1 — edit surface, loaded from npm bundle
- streamdown ^2.5.0 + @streamdown/mermaid 1.0.2 — agent/plan Markdown and bundled Mermaid SVG rendering
- radix-ui ^1.4.3 — shadcn/ui primitives
- lucide-react ^1.17.0 — icons
- class-variance-authority ^0.7.1, clsx ^2.1.1, tailwind-merge ^3.6.0 — styling utils
- use-stick-to-bottom ^1.1.6 — chat autoscroll
- @tanstack/react-virtual ^3.14.10 — variable-height conversation row virtualization
- (new) @tauri-apps/api ^2 — Tauri IPC client (invoke + event)

Dev: vite ^7.1.12, vitest ^3.2.6, typescript ~5.9.3, @vitejs/plugin-react ^5.0.4,
tailwindcss ^4.3.0, @tailwindcss/vite ^4.3.0, @testing-library/react ^16.3.2,
happy-dom ^16.8.1.

## Active Rust Stack (`src-tauri/Cargo.toml`)
- tauri 2 (stable) — desktop shell, WebView2 host, IPC, bundler/updater
- tauri-plugin-shell 2 — spawn the codex CLI subprocess
- tauri-plugin-dialog 2 — first-run editor-path picker
- tauri-winrt-notification 0.7 — branded Windows toast delivery and in-process activation callback
- windows-registry 0.5 — registers the app's per-user AppUserModelID display name and icon
- tokio 1 — async runtime (codex subprocess, file-IPC polling, downloads)
- fastembed 5.15 — bge-m3 ONNX embeddings (query-time); pulls `ort` (pykeio ONNX RT)
- rusqlite 0.32 — read the prebuilt RAG index (vectors + text + source metadata)
- reqwest 0.12 — bootstrap downloads (RAG index and version-matched Codex CLI/runtime helpers)
- sha2 0.10 — download and bundled adapter integrity verification
- base64 0.22 — EPSNAPSHOT UTF-8 project-path manifest decoding
- uuid 1 — collision-safe snapshot and analysis-directory request ownership
- parking_lot 0.12 — serialized preflight/analyzer process state
- windows-sys 0.59 — Job Object process-tree containment plus the Windows `MessageBeep` default
  notification sound used independently from native toast delivery
- similar 2 — unified diff (replaces Python difflib)
- image 0.25.10 with `default-features=false` and only png/jpeg/webp/gif — bounded Map Agent
  attachment metadata/decode, first-frame GIF normalization, Lanczos3 aspect-preserving resize
- which 8 — resolve codex and optional analyzer `node.exe` executables
- serde 1 + serde_json 1 — config/IPC/manifest/framed adapter payloads
- anyhow 1 + thiserror 1 — error handling
- bindgen 0.70 — generate FFI from `native/isom/isom_capi.h` (in `isom-sys`)

Map import uses existing `sha2`, `serde`, `parking_lot`, `uuid`, Tauri dialog/window APIs, and the
existing statically linked isom CHK/render/mapedit surface. No second map parser, copy engine, or
frontend rendering stack is introduced. External container bytes are streamed with a 256 MiB cap
into `%localappdata%\eud-agent\map_imports\blobs`; small strict project metadata stays under
`%appdata%\eud-agent\map_candidates\<project-id>\import-palette.json`.

Runtime toolchain: each saved session owns an official Codex CLI app-server client and ephemeral
loopback eud-tools MCP endpoint. Two strict elevated Windows profiles select read-only or
lease-owner write access to that session's isolated workspace; both disable sandboxed command
network access and avoid repository instructions. Codex hosted web search is explicitly `live`
and remains separate from that local process boundary.

## Build Artifacts
- tailwindcss v4.x (from `panel/dist` build via `@tailwindcss/vite`) — ground truth for
  the running panel CSS.
- `vendor/epscript-lsp-agent/adapter.cjs` — self-contained CommonJS adapter built with
  esbuild from `zuhanit/epscript-lsp@7f175df06ae57e9da65b8add25d084b5f5df0e1f`.
  Its SHA-256, MIT license, and provenance are separate bundled resources.

## Legacy / Vendored
- isom-poc C++ (`native/isom/`, vendored from isom-poc/IsomTerrain/) — MSBuild
  solution: IsomTerrain (lib) + CrossCutLib + IcuLib (vendored ICU) + CascLib. ABI v5 adds the
  packed bounded image quantizer over the existing cached CV5/VX4/VR4/WPE loader; palette
  construction, graphics validity, representative color, walkability, and height metadata remain
  native authority. The static `.lib` is linked into the Rust binary (Decision 09). Our repo is
  the source of truth; the editor's own C++ is never touched.
- vendor/webview2 — 3 WebView2 SDK DLLs from the POC; under Tauri the WebView2 runtime is
  the system Evergreen runtime, so these are retained only as a fallback reference.
- `vendor/epscript-lsp-agent` — generated, reviewable analyzer distribution. The build
  uses an exact npm lock and explicitly includes `@epscript-lsp/types@1.0.0`, omitted by
  the published server package metadata. Node core modules are the only externals.

## Removed / Superseded (deleted in v2)
- Python server stack (`server/`): fastapi, uvicorn, chromadb 1.5.9,
  sentence-transformers 5.5.1, transformers 5.10.1, torch 2.12.0, numpy 2.4.6,
  openai-codex 0.1.0b3, mcp 1.27.2 — all replaced by the Rust core. uv venv retired.
- In-editor WebView2 hosting + server-spawn lifecycle in the Lua bridge.

## Project Structure
- `src-tauri/` — Tauri app: session engine manager, project write coordinator, per-session tools/
  MCP/Codex drivers, canonical/session workspaces, journal, RAG, mapsafe, bridge I/O, preflight,
  memory, config, bootstrap, and CHK.
- `crates/isom-sys`, `crates/isom` — FFI bindings + safe wrapper for the C++ engine.
- `native/isom/` — vendored C++ + C ABI shim.
- `panel/` — React multi-entry app (`index.html`, `map-agent.html`, `map-import.html`) over Tauri
  IPC. Map Agent and the read-only importer inject candidate/import render sources into the same
  `MapCanvas`/`MapMinimap` implementations.
- `ci/` — RAG index builder + the committed corpus `ci/corpus/*.jsonl` (re-embeds the in-repo
  corpus with the runtime fastembed pipeline; output published to GitHub Releases).
- `tools/scraper/` — Node.js + TypeScript corpus tooling (local-only): authenticated Naver-Cafe
  API refresh plus commit-pinned SCRMapDocs/eudplib/eud-book/EUD Editor 3 extraction into
  `ci/corpus/*.jsonl`; its own package.json/tsconfig (TypeScript ~5.9, matching panel).
  HTTP, HTML parsing, cookie handling, and local Git snapshot dependencies are bound below.
- `tools/epscript-lsp-agent/` — TypeScript adapter surface, exact dependency lock,
  deterministic esbuild entry, and Node fixtures. `scripts/build_epscript_lsp_agent.ps1`
  fetches only the pinned/hash-verified archive and reproduces the committed vendor bytes.

## Rationale
- **Rust over Node/TS** remains the application decision: no Electron or in-process Node.
  The optional analyzer is a checksum-pinned, process-isolated adapter using an already
  installed `node.exe`; absence returns `skipped` and does not affect app readiness.
- **fastembed over candle** for embeddings: fastembed ships first-class bge-m3 ONNX with
  HF auto-download and quantized CPU models — less hand-rolling than candle for the same
  result.
- **rusqlite read-only index over chromadb**: chromadb is Python and mutates tracked
  sqlite on open (proven LFS churn); a CI-built read-only index avoids both.
- [BOUND 2026-06-08 from EUD-112-4f01] which 8.0.3 -- resolve the codex CLI shim path in codex_client (replaces Python shutil.which); honors CODEX_CMD override
- [BOUND 2026-06-08 from EUD-113-ba2a] similar 2 — TextDiff unified-diff generation for the engine instruct code/diff seam
- [BOUND 2026-06-09 from EUD-105-dba3] encoding_rs 0.8 — CHK string-table decode in src-tauri/src/chk.rs; EUC_KR == cp949 (WHATWG euc-kr index = unified hangul code), matching chk_info.py utf-8->cp949->latin-1 fallback for Korean map names
- [BOUND 2026-08-22] png 0.18 — encodes bounded RGB map-image previews and MCP minimap PNGs; no
  base64 JSON image payload is used for Tauri binary preview IPC
- [BOUND 2026-08-22] image 0.25.10, minimal png/jpeg/webp/gif codecs only — enforces Map Agent
  decode limits and normalizes source RGBA before the native deterministic tile quantizer
- [BOUND 2026-06-10 from EUD-138-db9b] undici ^7.16.0 — Naver-Cafe scraper HTTP client (tools/scraper, local-only cookie-gated; EUD-138)
- [BOUND 2026-06-10 from EUD-138-db9b] cheerio ^1.1.2 — Naver-Cafe scraper HTML parsing (tools/scraper post -> corpus row mapping; EUD-138)
- [BOUND 2026-06-12 from EUD-161-5726] ort =2.0.0-rc.12 — optional dependency gated ONLY by the ci `cuda` feature (local-only GPU differential-test track, never built in CI); pinned to exactly fastembed 5.16 transitive ort so cargo unifies the node and the default CPU build is unaffected
