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
- tauri-plugin-shell 2 — desktop shell integration; provider processes use Rust `Command`
- tauri-plugin-dialog 2 — first-run editor path and trusted attachment/map pickers
- tauri-winrt-notification 0.7 — branded Windows attention notifications
- windows-registry 0.5 — per-user AppUserModelID
- tokio 1 — per-provider HTTP streams, Codex/Claude subprocesses, OAuth callback, IPC polling
- fastembed 5.15 — bge-m3 ONNX embeddings
- rusqlite 0.32 — read-only prebuilt RAG index
- reqwest 0.12 with rustls — provider OAuth/catalog/inference and verified asset/CLI downloads
- sha2 0.10 — release, transcript-generation, source, and attachment integrity
- base64 0.22 — provider image blocks, OAuth state material, and bridge payloads
- uuid 1 — request/session/OAuth attempt ownership
- parking_lot 0.12 — short synchronous service/runtime state
- windows-sys 0.59 — Credential Manager, protected ACLs, Authenticode `WinVerifyTrust`, Job
  Objects, process discovery/memory, suspended-process control, background window input, and
  notification sound
- jsonschema 0.18 — Rust authority validation for compiler/harness structured output
- zeroize 1 — API key/token temporary buffer clearing
- similar 2 — unified diff
- image 0.25.10 with minimal png/jpeg/webp/gif codecs — bounded attachment/Map decode
- which 8 — configured/app-managed/PATH CLI and optional analyzer resolution
- serde 1 + serde_json 1 — strict provider/config/session/transcript/wire contracts
- anyhow 1 + thiserror 1 — error boundaries
- bindgen 0.70 — native isom C ABI generation

Map import uses existing `sha2`, `serde`, `parking_lot`, `uuid`, Tauri dialog/window APIs, and the
existing statically linked isom CHK/render/mapedit surface. No second map parser, copy engine, or
frontend rendering stack is introduced. External container bytes are streamed with a 256 MiB cap
into `%localappdata%\eud-agent\map_imports\blobs`; small strict project metadata stays under
`%appdata%\eud-agent\map_candidates\<project-id>\import-palette.json`.

Runtime provider set is closed: official Codex app-server and Claude Code subscription CLI use
app-owned profiles; Antigravity, OpenCode Go, and Ollama use direct Rust HTTP/SSE adapters.
OpenCode Go joins its live account catalog with OpenCode's public `models.dev` machine metadata;
Ollama uses a user-entered OpenAI-compatible base URL and model id without UI catalog enumeration.
Every variant shares `SessionToolRuntime` and an immutable session binding. Direct and optional
proxy secrets use Windows Credential Manager; direct histories use atomic hashed generations.
No OMP SDK/RPC, OpenCode server, Node provider runtime, unconfigured proxy, or silent
provider/model fallback is present.

## Build Artifacts
- tailwindcss v4.x (from `panel/dist` build via `@tailwindcss/vite`) — ground truth for
  the running panel CSS.
- `vendor/epscript-lsp-agent/adapter.cjs` — self-contained CommonJS adapter built with
  esbuild from `zuhanit/epscript-lsp@7f175df06ae57e9da65b8add25d084b5f5df0e1f`.
  Its SHA-256, MIT license, and provenance are separate bundled resources.

## Legacy / Vendored
- isom-poc C++ (`native/isom/`, vendored from isom-poc/IsomTerrain/) — MSBuild
  solution: IsomTerrain (lib) + CrossCutLib + IcuLib (vendored ICU) + CascLib. ABI v6 exposes
  the packed bounded image quantizer plus exact managed-sound add/replace entry points over the
  existing cached CV5/VX4/VR4/WPE and MappingCore loaders. Palette construction, graphics
  validity, representative color, walkability, height metadata, and CHK/MPQ deltas remain native
  authority. The static `.lib` is linked into the Rust binary (Decision 09). Our repo is
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
- `src-tauri/` — Tauri app: provider service/auth/CLI/direct-wire drivers/transcripts, session
  engine, provider-neutral tools/write coordinator/workspaces/journal/review, RAG/mapsafe/bridge/
  preflight/memory/config/bootstrap/CHK, plus the dependency-free Windows x86 trace runner and
  source-controlled `tests/**/*.tests.eps` suite discovery.
- `crates/isom-sys`, `crates/isom` — FFI bindings + safe wrapper for the C++ engine.
- `native/isom/` — vendored C++ + C ABI shim.
- `native/trace_injector.rs` — dependency-free x86 helper compiled and embedded by `src-tauri/build.rs`;
  validates the owned suspended SCR child and neutralizes only six fixed user32 focus/cursor exports.
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
