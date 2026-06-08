# eud-agent Tech Stack (v2 — Tauri + Rust)

Grounded against the repo on 2026-06-08. The v2 migration **removes the Python stack**,
**keeps the React panel**, and **adds a Rust/Tauri stack**. No `Cargo.toml` exists yet;
the Rust entries below are the target floor versions to pin at `cargo add` time.

## Active Dependencies (panel — kept, from `panel/package.json`)
- react 19.2.0 — panel UI
- react-dom 19.2.0 — DOM renderer
- @monaco-editor/react ^4.7.0 — Monaco React wrapper (CDN loader forbidden; bundled)
- monaco-editor ^0.55.1 — edit surface, loaded from npm bundle
- streamdown ^2.5.0 — agent markdown/stream rendering (AI Elements pipeline)
- radix-ui ^1.4.3 — shadcn/ui primitives
- lucide-react ^1.17.0 — icons
- class-variance-authority ^0.7.1, clsx ^2.1.1, tailwind-merge ^3.6.0 — styling utils
- use-stick-to-bottom ^1.1.6 — chat autoscroll
- (new) @tauri-apps/api ^2 — Tauri IPC client (invoke + event)

Dev: vite ^7.1.12, vitest ^3.2.6, typescript ~5.9.3, @vitejs/plugin-react ^5.0.4,
tailwindcss ^4.3.0, @tailwindcss/vite ^4.3.0, @testing-library/react ^16.3.2,
happy-dom ^16.8.1.

## Target Rust Stack (new — `src-tauri/Cargo.toml`, pin at add-time)
- tauri 2 (stable) — desktop shell, WebView2 host, IPC, bundler/updater
- tauri-plugin-shell 2 — spawn the codex CLI subprocess
- tauri-plugin-dialog 2 — first-run editor-path picker
- tokio 1 — async runtime (codex subprocess, file-IPC polling, downloads)
- fastembed 5.15 — bge-m3 ONNX embeddings (query-time); pulls `ort` (pykeio ONNX RT)
- rusqlite 0.32 — read the prebuilt RAG index (vectors + text + source metadata)
- reqwest 0.12 — first-run downloads (RAG index from GitHub Release)
- sha2 0.10 — download integrity verification
- similar 2 — unified diff (replaces Python difflib)
- which 7 — resolve the codex CLI shim path (replaces shutil.which)
- serde 1 + serde_json 1 — config/IPC/manifest (de)serialization
- anyhow 1 + thiserror 1 — error handling
- bindgen 0.70 — generate FFI from `native/isom/isom_capi.h` (in `isom-sys`)

## Build Artifacts
- tailwindcss v4.x (from `panel/dist` build via `@tailwindcss/vite`) — ground truth for
  the running panel CSS.

## Legacy / Vendored
- isom-poc C++ (`native/isom/`, vendored from `isom-poc/IsomTerrain/`) — MSBuild
  solution: IsomTerrain (lib) + CrossCutLib + IcuLib (vendored ICU) + CascLib. Built to a
  static `.lib` with a C ABI shim and linked into the Rust binary (Decision 09). Our repo
  is the source of truth; the editor's own C++ is never touched.
- vendor/webview2 — 3 WebView2 SDK DLLs from the POC; under Tauri the WebView2 runtime is
  the system Evergreen runtime, so these are retained only as a fallback reference.

## Removed / Superseded (deleted in v2)
- Python server stack (`server/`): fastapi, uvicorn, chromadb 1.5.9,
  sentence-transformers 5.5.1, transformers 5.10.1, torch 2.12.0, numpy 2.4.6,
  openai-codex 0.1.0b3, mcp 1.27.2 — all replaced by the Rust core. uv venv retired.
- In-editor WebView2 hosting + server-spawn lifecycle in the Lua bridge.

## Project Structure
- `src-tauri/` — Tauri Rust app (core modules: ipc, engine, tools, codex_client, rag,
  isom, mapsafe, bridge_io, memory, config, bootstrap, chk).
- `crates/isom-sys`, `crates/isom` — FFI bindings + safe wrapper for the C++ engine.
- `native/isom/` — vendored C++ + C ABI shim.
- `panel/` — React app (reused), Tauri IPC transport; built to `panel/dist`.
- `ci/` — RAG index builder (re-embeds the ECA corpus with the runtime fastembed
  pipeline; output published to GitHub Releases).

## Rationale
- **Rust over Node/TS** (Decision 08): the small-distributable goal and the safety of the
  map binary path outweigh Node's more mature ML ecosystem; Electron's only edge
  (in-process node for the advisory LSP) does not justify a ~150MB bundle.
- **fastembed over candle** for embeddings: fastembed ships first-class bge-m3 ONNX with
  HF auto-download and quantized CPU models — less hand-rolling than candle for the same
  result.
- **rusqlite read-only index over chromadb**: chromadb is Python and mutates tracked
  sqlite on open (proven LFS churn); a CI-built read-only index avoids both.
